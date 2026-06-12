use directories::ProjectDirs;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use crate::engines::audio_engine::store::store_api_client::{
    ModuleStatus, RemoteAudioModuleInfo, StoreApiClient, StoreApiError, 
    StoreDownloadProgressListener, RustDownloadProgressHandler,
};

// ─── LOCAL TRAIT IMPLEMENTOR STRUCT ───
// We define a lightweight structural container to bridge the progress closure 
// into the pure-Rust `RustDownloadProgressHandler` trait boundary smoothly.
struct ServiceProgressProxy {
    module_id: String,
    cached_modules: Arc<Mutex<Vec<RemoteAudioModuleInfo>>>,
    upstream_listener: Arc<StoreDownloadProgressListener>,
}

impl RustDownloadProgressHandler for ServiceProgressProxy {
    fn on_progress(&self, unique_id: String, bytes_written: u64, total_bytes: Option<u64>) {
        let progress = match total_bytes {
            Some(total) if total > 0 => bytes_written as f64 / total as f64,
            _ => 0.0,
        };

        // 1. Update our internal repository cache layers
        if let Ok(mut cache) = self.cached_modules.lock() {
            if let Some(module) = cache.iter_mut().find(|m| m.unique_id == self.module_id) {
                module.status = ModuleStatus::Downloading { progress };
            }
        }

        // 2. FIX: Call the public .on_progress method instead of looking for an absent .callback field
        self.upstream_listener.on_progress(unique_id, bytes_written, total_bytes);
    }
}

#[derive(uniffi::Object)]
pub struct StoreApiService {
    endpoint_url: String,
    api_client: OnceLock<Arc<StoreApiClient>>,
    cached_modules: Arc<Mutex<Vec<RemoteAudioModuleInfo>>>,
}

#[uniffi::export]
impl StoreApiService {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self {
            endpoint_url: "https://ap-south-1.cdn.hygraph.com/content/cmpwxdh8104yx07w6h1ffpokb/master".into(),
            api_client: OnceLock::new(),
            cached_modules: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn get_client(&self) -> Arc<StoreApiClient> {
        self.api_client
            .get_or_init(|| {
                Arc::new(StoreApiClient::new(self.endpoint_url.clone(), None))
            })
            .clone()
    }

    pub fn get_audio_modules_path(&self) -> String {
        let proj_dirs = ProjectDirs::from("org", "flame", "xbible").expect("Path error");
        let path = proj_dirs.data_local_dir().join("modules").join("audio");
        fs::create_dir_all(&path).ok();
        path.to_string_lossy().into_owned()
    }

    pub async fn load_catalog(&self) -> Result<Vec<RemoteAudioModuleInfo>, StoreApiError> {
        let client = self.get_client();
        let mut modules = client.fetch_audio_modules().await?;
        
        let target_dir = self.get_audio_modules_path();
        let target_path = Path::new(&target_dir);

        for module in &mut modules {
            let file_name = format!("{}_v{}.xba", module.unique_id, module.version);
            module.status = if target_path.join(file_name).exists() {
                ModuleStatus::Installed
            } else {
                ModuleStatus::Idle
            };
        }

        if let Ok(mut cache) = self.cached_modules.lock() {
            *cache = modules.clone();
        }

        Ok(modules)
    }

    pub fn get_cached_catalog(&self) -> Vec<RemoteAudioModuleInfo> {
        if let Ok(cache) = self.cached_modules.lock() {
            cache.clone()
        } else {
            vec![]
        }
    }

    pub async fn install_module(
        &self,
        module_id: String,
        progress_listener: Arc<StoreDownloadProgressListener>,
    ) -> Result<String, StoreApiError> {
        let target_module = {
            let cache = self.cached_modules.lock().unwrap();
            cache.iter().find(|m| m.unique_id == module_id).cloned()
        }
        .ok_or_else(|| StoreApiError::SerializationFailure {
            message: format!("Module {} not found in cache.", module_id),
        })?;

        let target_dir = self.get_audio_modules_path();
        
        // Build our concrete structure type instance instead of a raw closure block
        let proxy_handler = Arc::new(ServiceProgressProxy {
            module_id: module_id.clone(),
            cached_modules: Arc::clone(&self.cached_modules),
            upstream_listener: Arc::clone(&progress_listener),
        });

        // Use our non-FFI constructor to securely link the progress listener trait
        let final_proxy_listener = Arc::new(StoreDownloadProgressListener::new_native(
            Some(module_id.clone()),
            proxy_handler,
        ));

        let client = self.get_client();
        let saved_path = client
            .download_and_install_module(target_module, target_dir, final_proxy_listener)
            .await?;

        if let Ok(mut cache) = self.cached_modules.lock() {
            if let Some(module) = cache.iter_mut().find(|m| m.unique_id == module_id) {
                module.status = ModuleStatus::Installed;
                module.is_installed = true;
            }
        }

        Ok(saved_path)
    }
}