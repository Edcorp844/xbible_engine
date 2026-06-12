use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};

#[derive(Debug, Serialize, Deserialize, Clone, uniffi::Record)]
pub struct HygraphFeatures {
    pub features: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, uniffi::Record)]
pub struct RemoteArtworkFile {
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, uniffi::Record)]
pub struct HygraphAudioResponse {
    #[serde(rename = "xBibleAudioModules")]
    pub audio_modules: Vec<RemoteAudioModuleInfo>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, uniffi::Enum)]
pub enum ModuleStatus {
    #[default]
    Idle,
    Downloading {
        progress: f64,
    },
    Installed,
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
pub struct RemoteAudioModuleInfo {
    #[serde(rename = "uniqueId")]
    pub unique_id: String,
    #[serde(rename = "displayTitle")]
    pub display_title: String,
    pub contributor: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "durationMs")]
    pub duration_ms: i64,
    pub features: HygraphFeatures,
    pub language: String,
    #[serde(rename = "sourceUrl")]
    pub source_url: String,
    #[serde(rename = "artworkFile")]
    pub artwork_file: Option<RemoteArtworkFile>,
    pub version: i32,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(default)]
    pub is_installed: bool,
    #[serde(skip)]
    pub status: ModuleStatus,
}

impl Default for RemoteAudioModuleInfo {
    fn default() -> Self {
        Self {
            unique_id: String::new(),
            display_title: String::new(),
            contributor: None,
            description: None,
            duration_ms: 0,
            features: HygraphFeatures { features: vec![] },
            language: String::new(),
            source_url: String::new(),
            artwork_file: None,
            version: 1,
            created_at: String::new(),
            updated_at: String::new(),
            is_installed: false,
            status: ModuleStatus::Idle,
        }
    }
}

#[derive(uniffi::Object)]
pub struct StoreApiClient {
    client: Client,
    endpoint_url: String,
    auth_token: Option<String>,
}

#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
pub enum StoreApiError {
    #[error("Network infrastructure failure: {message}")]
    NetworkFailure { message: String },
    #[error("API Deserialization payload error: {message}")]
    SerializationFailure { message: String },
    #[error("Local filesystem file I/O layout drop: {message}")]
    IoFailure { message: String },
}

pub trait RustDownloadProgressHandler: Send + Sync {
    fn on_progress(&self, unique_id: String, bytes_written: u64, total_bytes: Option<u64>);
}

#[derive(uniffi::Object)]
pub struct StoreDownloadProgressListener {
    pub unique_id_filter: Option<String>,
    pub rust_handler: Option<Arc<dyn RustDownloadProgressHandler>>,
}

#[uniffi::export]
impl StoreDownloadProgressListener {
    #[uniffi::constructor]
    pub fn new(unique_id_filter: Option<String>) -> Self {
        Self { 
            unique_id_filter,
            rust_handler: None, 
        }
    }

    pub fn on_progress(&self, unique_id: String, bytes_written: u64, total_bytes: Option<u64>) {
        if let Some(ref handler) = self.rust_handler {
            handler.on_progress(unique_id, bytes_written, total_bytes);
        }
    }
}

impl StoreDownloadProgressListener {
    pub fn new_native(
        unique_id_filter: Option<String>, 
        handler: Arc<dyn RustDownloadProgressHandler>
    ) -> Self {
        Self {
            unique_id_filter,
            rust_handler: Some(handler),
        }
    }
}

#[uniffi::export]
impl StoreApiClient {
    #[uniffi::constructor]
    pub fn new(endpoint_url: String, auth_token: Option<String>) -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::CONTENT_TYPE,
            reqwest::header::HeaderValue::from_static("application/json"),
        );
        headers.insert(
            reqwest::header::USER_AGENT,
            reqwest::header::HeaderValue::from_static("X-Bible-Engine/1.0"),
        );

        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(300)) // Generous timeout for large modules
            .http1_only()                      // Prevents HTTP/2 multiplex frame drops
            .tcp_keepalive(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            endpoint_url,
            auth_token,
        }
    }

    pub async fn fetch_audio_modules(&self) -> Result<Vec<RemoteAudioModuleInfo>, StoreApiError> {
        let query = r#"
            query MyQuery {
              xBibleAudioModules {
                artworkFile { url }
                contributor
                description
                createdAt
                displayTitle
                durationMs
                features
                language
                sourceUrl
                uniqueId
                updatedAt
                version
              }
            }
        "#;

        let payload = serde_json::json!({ "query": query });
        let mut request = self.client.post(&self.endpoint_url).json(&payload);

        if let Some(ref token) = self.auth_token {
            request = request.header("Authorization", format!("Bearer {}", token));
        }

        let response = request
            .send()
            .await
            .map_err(|e| StoreApiError::NetworkFailure { message: e.to_string() })?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| StoreApiError::SerializationFailure { message: e.to_string() })?;

        let data_section = response.get("data").ok_or_else(|| {
            StoreApiError::SerializationFailure { message: "No 'data' field".to_string() }
        })?;

        let parsed: HygraphAudioResponse = serde_json::from_value(data_section.clone()).map_err(|e| {
            StoreApiError::SerializationFailure { message: e.to_string() }
        })?;

        Ok(parsed.audio_modules)
    }

    pub async fn download_and_install_module(
        &self,
        module: RemoteAudioModuleInfo,
        target_dir_path: String,
        progress_listener: Arc<StoreDownloadProgressListener>,
    ) -> Result<String, StoreApiError> {
        let target_dir = Path::new(&target_dir_path);
        let file_name = format!("{}_v{}.xba", module.unique_id, module.version);
        let destination_path = target_dir.join(file_name);

        if destination_path.exists() {
            if let Ok(meta) = std::fs::metadata(&destination_path) {
                progress_listener.on_progress(
                    module.unique_id.clone(),
                    meta.len(),
                    Some(meta.len()),
                );
            }
            return Ok(destination_path.to_string_lossy().into_owned());
        }

        // 1. Establish the connection handle forcing un-adulterated raw streams
        let mut response = self
            .client
            .get(&module.source_url)
            .header(reqwest::header::ACCEPT_ENCODING, "identity")
            .send()
            .await
            .map_err(|e| StoreApiError::NetworkFailure {
                message: format!("GB Handshake initialization failure: {}", e),
            })?;

        let total_size = response.content_length();
        
        // Scale our progress reporting threshold dynamically for large multi-gigabyte assets
        let chunk_threshold = match total_size {
            Some(total) if total > 200 => total / 200, // 0.5% granular steps
            _ => 64 * 1024,
        };

        // 2. FIX: Create a tightly bounded channel (16 slots)
        // This caps maximum memory overhead to a few megabytes, completely preventing OOM crashes.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(16);

        // 3. SEPARATED ASYNC NETWORK PUMP
        tokio::spawn(async move {
            while let Ok(Some(chunk)) = response.chunk().await {
                // If the disk loop drops or error terminates, break network pipeline cleanly
                if tx.send(chunk).await.is_err() {
                    break;
                }
            }
            drop(tx);
        });

        // 4. SEQUENTIAL DISK CONSUMER & TELEMETRY STREAM
        let file = File::create(&destination_path)
            .await
            .map_err(|e| StoreApiError::IoFailure {
                message: format!("Failed to create local volume asset: {}", e),
            })?;
        
        // Bump writer buffer capacity to 256KB to reduce physical SSD flush iterations
        let mut buffered_writer = BufWriter::with_capacity(256 * 1024, file);

        let mut bytes_written: u64 = 0;
        let mut last_reported_bytes: u64 = 0;
        let mut last_update_time = Instant::now();
        let throttle_interval = Duration::from_millis(250); // 250ms update cadence keeps UI stable

        println!("Streaming high-volume asset profile: {}", module.display_title);

        while let Some(chunk) = rx.recv().await {
            buffered_writer
                .write_all(&chunk)
                .await
                .map_err(|e| StoreApiError::IoFailure {
                    message: format!("Disk storage partition write failure: {}", e),
                })?;

            bytes_written += chunk.len() as u64;

            let bytes_since_report = bytes_written.saturating_sub(last_reported_bytes);
            let time_since_report = last_update_time.elapsed();

            if bytes_since_report >= chunk_threshold || time_since_report >= throttle_interval {
                // let percentage = match total_size {
                //     Some(total) if total > 0 => (bytes_written as f64 / total as f64) * 100.0,
                //     _ => 0.0,
                // };

                // Print directly to terminal instantly
                // println!(
                //     "[{}] Downloading: {:.2}% ({}/{:?} bytes)",
                //     module.unique_id, percentage, bytes_written, total_size
                // );

                // Pass metrics out to UniFFI/Swift pool without stalling the disk processing context
                let listener_clone = Arc::clone(&progress_listener);
                let id_clone = module.unique_id.clone();
                tokio::spawn(async move {
                    listener_clone.on_progress(id_clone, bytes_written, total_size);
                });

                last_reported_bytes = bytes_written;
                last_update_time = Instant::now();
            }
        }

        // 5. Hard flush the disk structure
        buffered_writer
            .flush()
            .await
            .map_err(|e| StoreApiError::IoFailure {
                message: format!("Failed to finalize cache disk allocations: {}", e),
            })?;

        println!("[{}] Large asset download completed successfully!", module.unique_id);
        progress_listener.on_progress(module.unique_id.clone(), bytes_written, total_size);

        Ok(destination_path.to_string_lossy().into_owned())
    }
}