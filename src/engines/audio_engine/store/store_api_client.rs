use futures_util::StreamExt;
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

// ─── FFI-COMPLIANT PROGRESSION PROXY OBJECT ───

#[derive(uniffi::Object)]
pub struct StoreDownloadProgressListener {
    // We drop the Box<dyn Fn> completely to make UniFFI happy
    pub unique_id_filter: Option<String>,
}

#[uniffi::export]
impl StoreDownloadProgressListener {
    #[uniffi::constructor]
    pub fn new(unique_id_filter: Option<String>) -> Self {
        Self { unique_id_filter }
    }

    // This method is called from Swift or Rust to dispatch telemetry updates
    pub fn on_progress(&self, unique_id: String, bytes_written: u64, total_bytes: Option<u64>) {
        // Handled natively via UniFFI messaging patterns or overridden on the foreign language boundary
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
            .timeout(Duration::from_secs(60))
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
            .map_err(|e| StoreApiError::NetworkFailure {
                message: e.to_string(),
            })?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| StoreApiError::SerializationFailure {
                message: e.to_string(),
            })?;

        let data_section =
            response
                .get("data")
                .ok_or_else(|| StoreApiError::SerializationFailure {
                    message: "No 'data' field in response".to_string(),
                })?;

        let parsed: HygraphAudioResponse =
            serde_json::from_value(data_section.clone()).map_err(|e| {
                StoreApiError::SerializationFailure {
                    message: e.to_string(),
                }
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

        let response = self
            .client
            .get(&module.source_url)
            .send()
            .await
            .map_err(|e| StoreApiError::NetworkFailure {
                message: e.to_string(),
            })?;

        let total_size = response.content_length();
        let chunk_threshold = match total_size {
            Some(total) if total > 100 => total / 100,
            _ => 32 * 1024,
        };

        let file = File::create(&destination_path)
            .await
            .map_err(|e| StoreApiError::IoFailure {
                message: e.to_string(),
            })?;
        let mut buffered_writer = BufWriter::with_capacity(32 * 1024, file);

        let mut byte_stream = response.bytes_stream();
        let mut bytes_written: u64 = 0;
        let mut last_reported_bytes: u64 = 0;
        let mut last_update_time = Instant::now();
        let throttle_interval = Duration::from_millis(250);

        while let Some(chunk_result) = byte_stream.next().await {
            let chunk = chunk_result.map_err(|e| StoreApiError::NetworkFailure {
                message: format!("Download chunk stream split failure: {}", e),
            })?;

            buffered_writer
                .write_all(&chunk)
                .await
                .map_err(|e| StoreApiError::IoFailure {
                    message: e.to_string(),
                })?;

            bytes_written += chunk.len() as u64;

            let bytes_since_report = bytes_written.saturating_sub(last_reported_bytes);
            let time_since_report = last_update_time.elapsed();

            if bytes_since_report >= chunk_threshold || time_since_report >= throttle_interval {
                progress_listener.on_progress(module.unique_id.clone(), bytes_written, total_size);
                last_reported_bytes = bytes_written;
                last_update_time = Instant::now();
            }
        }

        buffered_writer
            .flush()
            .await
            .map_err(|e| StoreApiError::IoFailure {
                message: format!("Failed to flush target cache disk buffers: {}", e),
            })?;

        progress_listener.on_progress(module.unique_id.clone(), bytes_written, total_size);

        Ok(destination_path.to_string_lossy().into_owned())
    }
}