// src/engines/audio_engine/store/store_api_service.rs

use directories::ProjectDirs;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use crate::engines::audio_engine::store::local_store::{ModuleLocation, read_stamped_metadata};
use crate::engines::audio_engine::store::store_api_client::{
    AudioAlbum, AudioCollection, AudioContributor, AudioLanguage, ContributorRole,
    DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, ModuleStatus, RemoteAudioModuleInfo,
    RustDownloadProgressHandler, StoreApiClient, StoreApiError, StoreDownloadProgressListener,
};

use crate::runtime::get_shared_runtime;

// ─── PROGRESS PROXY ──────────────────────────────────────────────────────────

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

        if let Ok(mut cache) = self.cached_modules.lock() {
            if let Some(module) = cache.iter_mut().find(|m| m.unique_id == self.module_id) {
                module.status = ModuleStatus::Downloading { progress };
            }
        }

        self.upstream_listener
            .on_progress(unique_id, bytes_written, total_bytes);
    }
}

// ─── SERVICE ─────────────────────────────────────────────────────────────────

#[derive(uniffi::Object)]
pub struct StoreApiService {
    endpoint_url: String,
    api_client: OnceLock<Arc<StoreApiClient>>,
    cached_modules: Arc<Mutex<Vec<RemoteAudioModuleInfo>>>,
}

impl StoreApiService {
    fn get_client(&self) -> Arc<StoreApiClient> {
        self.api_client
            .get_or_init(|| Arc::new(StoreApiClient::new(self.endpoint_url.clone(), None)))
            .clone()
    }

    fn run_blocking<T>(
        &self,
        fut: impl std::future::Future<Output = Result<T, StoreApiError>>,
    ) -> Result<T, StoreApiError> {
        get_shared_runtime().block_on(fut)
    }

    fn clamp(first: i32, skip: i32) -> (i32, i32) {
        (first.clamp(1, MAX_PAGE_SIZE), skip.max(0))
    }

    fn read_cache(&self) -> Vec<RemoteAudioModuleInfo> {
        self.cached_modules
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default()
    }

    fn write_cache(&self, modules: Vec<RemoteAudioModuleInfo>) {
        if let Ok(mut cache) = self.cached_modules.lock() {
            *cache = modules;
        }
    }

    fn mark_installed_state(&self, modules: &mut [RemoteAudioModuleInfo]) {
        let root = self.get_audio_modules_path();
        let root_path = Path::new(&root);

        let mut installed: Vec<(String, i32)> = Vec::new();
        for entry in walkdir::WalkDir::new(root_path)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("xba") {
                continue;
            }
            if let Ok(meta) = read_stamped_metadata(path) {
                if !meta.organization.stamped_at.is_empty() {
                    installed.push((meta.stub.unique_id, meta.stub.version));
                }
            }
        }

        for module in modules.iter_mut() {
            let match_found = installed
                .iter()
                .any(|(uid, ver)| uid == &module.unique_id && *ver == module.version);
            module.status = if match_found {
                ModuleStatus::Installed
            } else {
                ModuleStatus::Idle
            };
            module.is_installed = match_found;
        }
    }

    /// Build the full hierarchy context for a module.
    /// The key fix: clone `collection.id` before the async move so the
    /// outer `collection` remains owned for the return tuple.
    fn resolve_hierarchy(
        &self,
        module: &RemoteAudioModuleInfo,
    ) -> Result<
        (
            AudioLanguage,
            AudioCollection,
            AudioAlbum,
            Option<AudioContributor>,
        ),
        StoreApiError,
    > {
        let client = self.get_client();

        // 1. Find the album. Fast path uses `module.album_id`; only falls
        //    back to scanning when the module has no album reference.
        let album = match module.album_id.as_ref().map(|r| r.id.clone()) {
            Some(album_id) => self.run_blocking({
                let client = client.clone();
                async move { client.fetch_album(album_id).await }
            })?,
            None => self.run_blocking({
                let client = client.clone();
                let wanted = module.unique_id.clone();
                async move {
                    let albums = client.fetch_albums(MAX_PAGE_SIZE, 0).await?;
                    for a in albums {
                        let full = client.fetch_album(a.id.clone()).await?;
                        if full.modules.iter().any(|m| m.unique_id == wanted) {
                            return Ok::<_, StoreApiError>(full);
                        }
                    }
                    Err(StoreApiError::NotFound {
                        id: format!("module {} not in any album", wanted),
                    })
                }
            })?,
        };

        // 2. Collection.
        let collection_id = album
            .collection_id
            .as_ref()
            .map(|r| r.id.clone())
            .ok_or_else(|| StoreApiError::NotFound {
                id: format!("album {} has no collection", album.id),
            })?;
        let collection = self.run_blocking({
            let client = client.clone();
            async move { client.fetch_collection(collection_id).await }
        })?;

        // 3. Language.
        let language_id = collection
            .language_id
            .as_ref()
            .map(|r| r.id.clone())
            .ok_or_else(|| StoreApiError::NotFound {
                id: format!("collection {} has no language", collection.id),
            })?;
        let language = self.run_blocking({
            let client = client.clone();
            async move { client.fetch_language(language_id).await }
        })?;

        let contributor = self
            .run_blocking({
                let client = client.clone();
                let coll_id = collection.id.clone();
                async move { client.fetch_narrators_for_collection(coll_id).await }
            })
            .ok()
            .and_then(|v| v.into_iter().next());

        Ok((language, collection, album, contributor))
    }
}

#[uniffi::export]
impl StoreApiService {
    #[uniffi::constructor]
    pub fn new() -> Self {
        Self {
            endpoint_url:
                "https://ap-south-1.cdn.hygraph.com/content/cmpwxdh8104yx07w6h1ffpokb/master".into(),
            api_client: OnceLock::new(),
            cached_modules: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn get_audio_modules_path(&self) -> String {
        let proj_dirs = ProjectDirs::from("org", "flame", "xbible").expect("Path error");
        let path = proj_dirs.data_local_dir().join("modules").join("audio");
        fs::create_dir_all(&path).ok();
        path.to_string_lossy().into_owned()
    }

    // ── Catalog ───────────────────────────────────────────────────────────

    pub fn load_audio_modules(
        &self,
        first: i32,
        skip: i32,
    ) -> Result<Vec<RemoteAudioModuleInfo>, StoreApiError> {
        let (first, skip) = Self::clamp(first, skip);
        let client = self.get_client();
        let mut modules =
            self.run_blocking(async move { client.fetch_audio_modules(first, skip).await })?;
        self.mark_installed_state(&mut modules);
        self.write_cache(modules.clone());
        Ok(modules)
    }

    pub fn load_catalog(&self) -> Result<Vec<RemoteAudioModuleInfo>, StoreApiError> {
        self.load_audio_modules(DEFAULT_PAGE_SIZE, 0)
    }

    pub fn get_cached_catalog(&self) -> Vec<RemoteAudioModuleInfo> {
        self.read_cache()
    }

    // ── Browsing ──────────────────────────────────────────────────────────

    pub fn fetch_languages(
        &self,
        first: i32,
        skip: i32,
    ) -> Result<Vec<AudioLanguage>, StoreApiError> {
        let (first, skip) = Self::clamp(first, skip);
        let client = self.get_client();
        self.run_blocking(async move { client.fetch_languages(first, skip).await })
    }

    pub fn fetch_languages_with_collections(
        &self,
        first: i32,
        skip: i32,
    ) -> Result<Vec<AudioLanguage>, StoreApiError> {
        let (first, skip) = Self::clamp(first, skip);
        let client = self.get_client();
        self.run_blocking(async move { client.fetch_languages_with_collections(first, skip).await })
    }

    pub fn fetch_language(&self, id: String) -> Result<AudioLanguage, StoreApiError> {
        let client = self.get_client();
        self.run_blocking(async move { client.fetch_language(id).await })
    }

    pub fn fetch_collections(
        &self,
        first: i32,
        skip: i32,
    ) -> Result<Vec<AudioCollection>, StoreApiError> {
        let (first, skip) = Self::clamp(first, skip);
        let client = self.get_client();
        self.run_blocking(async move { client.fetch_collections(first, skip).await })
    }

    pub fn fetch_collections_with_albums(
        &self,
        first: i32,
        skip: i32,
    ) -> Result<Vec<AudioCollection>, StoreApiError> {
        let (first, skip) = Self::clamp(first, skip);
        let client = self.get_client();
        self.run_blocking(async move { client.fetch_collections_with_albums(first, skip).await })
    }

    pub fn fetch_collections_by_language(
        &self,
        code: String,
        first: i32,
        skip: i32,
    ) -> Result<Vec<AudioCollection>, StoreApiError> {
        let (first, skip) = Self::clamp(first, skip);
        let client = self.get_client();
        self.run_blocking(async move {
            client
                .fetch_collections_by_language(code, first, skip)
                .await
        })
    }

    pub fn fetch_collection(&self, id: String) -> Result<AudioCollection, StoreApiError> {
        let client = self.get_client();
        self.run_blocking(async move { client.fetch_collection(id).await })
    }

    pub fn fetch_albums(&self, first: i32, skip: i32) -> Result<Vec<AudioAlbum>, StoreApiError> {
        let (first, skip) = Self::clamp(first, skip);
        let client = self.get_client();
        self.run_blocking(async move { client.fetch_albums(first, skip).await })
    }

    pub fn fetch_albums_by_collection(
        &self,
        collection_id: String,
        first: i32,
        skip: i32,
    ) -> Result<Vec<AudioAlbum>, StoreApiError> {
        let (first, skip) = Self::clamp(first, skip);
        let client = self.get_client();
        self.run_blocking(async move {
            client
                .fetch_albums_by_collection(collection_id, first, skip)
                .await
        })
    }

    pub fn fetch_album(&self, id: String) -> Result<AudioAlbum, StoreApiError> {
        let client = self.get_client();
        self.run_blocking(async move { client.fetch_album(id).await })
    }

    pub fn fetch_contributors(
        &self,
        first: i32,
        skip: i32,
    ) -> Result<Vec<AudioContributor>, StoreApiError> {
        let (first, skip) = Self::clamp(first, skip);
        let client = self.get_client();
        self.run_blocking(async move { client.fetch_contributors(first, skip).await })
    }

    pub fn fetch_contributors_by_role(
        &self,
        role: ContributorRole,
        first: i32,
        skip: i32,
    ) -> Result<Vec<AudioContributor>, StoreApiError> {
        let (first, skip) = Self::clamp(first, skip);
        let client = self.get_client();
        self.run_blocking(async move { client.fetch_contributors_by_role(role, first, skip).await })
    }

    pub fn fetch_contributor(&self, id: String) -> Result<AudioContributor, StoreApiError> {
        let client = self.get_client();
        self.run_blocking(async move { client.fetch_contributor(id).await })
    }

    // ── Install ───────────────────────────────────────────────────────────

    pub fn install_module(
        &self,
        module_id: String,
        progress_listener: Arc<StoreDownloadProgressListener>,
    ) -> Result<String, StoreApiError> {
        let target_module = {
            let cache =
                self.cached_modules
                    .lock()
                    .map_err(|_| StoreApiError::SerializationFailure {
                        error_message: "Catalog cache poisoned".into(),
                    })?;
            cache
                .iter()
                .find(|m| m.unique_id == module_id || m.id.as_deref() == Some(module_id.as_str()))
                .cloned()
        }
        .ok_or_else(|| StoreApiError::SerializationFailure {
            error_message: format!("Module {} not found in cache.", module_id),
        })?;

        let resolved_id = target_module.unique_id.clone();

        let (language, collection, album, contributor) = self
            .resolve_hierarchy(&target_module)
            .map_err(|e| StoreApiError::SerializationFailure {
                error_message: format!("Could not resolve hierarchy: {:?}", e),
            })?;

        log::info!(
            "resolved contributor: name={:?} photo_url={:?}",
            contributor.as_ref().map(|c| c.name.as_str()),
            contributor
                .as_ref()
                .and_then(|c| c.photo.as_ref().map(|p| p.url.as_str())),
        );

        let location = ModuleLocation::from_hierarchy(
            &language,
            &collection,
            &album,
            contributor.as_ref(),
            &target_module,
        );

        let target_dir = self.get_audio_modules_path();

        let proxy_handler = Arc::new(ServiceProgressProxy {
            module_id: resolved_id.clone(),
            cached_modules: Arc::clone(&self.cached_modules),
            upstream_listener: Arc::clone(&progress_listener),
        });

        let final_proxy_listener = Arc::new(StoreDownloadProgressListener::new_native(
            Some(resolved_id.clone()),
            proxy_handler,
        ));

        let client = self.get_client();
        let saved_path = self.run_blocking(async move {
            client
                .download_and_install_module(
                    target_module,
                    target_dir,
                    location,
                    collection,
                    album,
                    contributor,
                    final_proxy_listener,
                )
                .await
        })?;

        if let Ok(mut cache) = self.cached_modules.lock() {
            if let Some(module) = cache.iter_mut().find(|m| m.unique_id == resolved_id) {
                module.status = ModuleStatus::Installed;
                module.is_installed = true;
            }
        }

        Ok(saved_path)
    }

    // ── Installed-only view ───────────────────────────────────────────────

    pub fn load_installed_modules(&self) -> Vec<RemoteAudioModuleInfo> {
        let root = self.get_audio_modules_path();
        crate::engines::audio_engine::store::local_store::scan_installed_modules(Path::new(&root))
    }

    pub fn is_module_installed(&self, module_id: String, version: i32) -> bool {
        let root = self.get_audio_modules_path();
        let root_path = Path::new(&root);
        for entry in walkdir::WalkDir::new(root_path)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("xba") {
                continue;
            }
            if let Ok(meta) = read_stamped_metadata(path) {
                if meta.stub.unique_id == module_id && meta.stub.version == version {
                    return !meta.organization.stamped_at.is_empty();
                }
            }
        }
        false
    }
}
