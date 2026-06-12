use directories::ProjectDirs;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::engines::audio_engine::store::store_api_client::{
    ModuleStatus, RemoteAudioModuleInfo, StoreApiClient, StoreApiError, StoreDownloadProgressListener,
};

#[derive(uniffi::Object)]
pub struct StoreApiService {
    api_client: StoreApiClient,
    cached_modules: Arc<Mutex<Vec<RemoteAudioModuleInfo>>>,
}

#[uniffi::export]
impl StoreApiService {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self {
            api_client: StoreApiClient::new(
                "https://ap-south-1.cdn.hygraph.com/content/cmpwxdh8104yx07w6h1ffpokb/master".into(),
                None,
            ),
            cached_modules: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn get_audio_modules_path(&self) -> String {
        let proj_dirs = ProjectDirs::from("org", "flame", "xbible").expect("Path error");
        let path = proj_dirs.data_local_dir().join("modules").join("audio");
        fs::create_dir_all(&path).ok();
        path.to_string_lossy().into_owned()
    }

    pub async fn load_catalog(&self) -> Result<Vec<RemoteAudioModuleInfo>, StoreApiError> {
        let mut modules = self.api_client.fetch_audio_modules().await?;
        
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

    // Polling mechanism update loop hook
    pub fn update_progress_manually(&self, module_id: String, bytes_written: u64, total_bytes: Option<u64>) {
        let progress = match total_bytes {
            Some(total) if total > 0 => bytes_written as f64 / total as f64,
            _ => 0.0,
        };

        if let Ok(mut cache) = self.cached_modules.lock() {
            if let Some(module) = cache.iter_mut().find(|m| m.unique_id == module_id) {
                if progress >= 1.0 {
                    module.status = ModuleStatus::Installed;
                    module.is_installed = true;
                } else {
                    module.status = ModuleStatus::Downloading { progress };
                }
            }
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

        let saved_path = self
            .api_client
            .download_and_install_module(target_module, target_dir, progress_listener)
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