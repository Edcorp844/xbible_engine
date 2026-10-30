// src/api/mod.rs
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncWriteExt, BufWriter};

use crate::engines::audio_engine::store::local_store::{
    ModuleLocation, XbaMetadata, read_stamped_metadata, read_stub_metadata, rewrite_xba_with_stamp,
};

// ─────────────────────────────────────────────────────────────────────────────
// Pagination constants
// ─────────────────────────────────────────────────────────────────────────────

pub const MAX_PAGE_SIZE: i32 = 100;
pub const DEFAULT_PAGE_SIZE: i32 = 50;

fn clamp_page(first: i32, skip: i32) -> (i32, i32) {
    (first.clamp(1, MAX_PAGE_SIZE), skip.max(0))
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared reference helpers
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone, uniffi::Record)]
pub struct IdRef {
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, uniffi::Record)]
pub struct RichTextValue {
    #[serde(default)]
    pub text: String,
}

impl RichTextValue {
    pub fn as_string(&self) -> String {
        self.text.clone()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Models
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone, uniffi::Record)]
pub struct HygraphFeatures {
    #[serde(default)]
    pub features: Vec<String>,
}

impl Default for HygraphFeatures {
    fn default() -> Self {
        Self { features: vec![] }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, uniffi::Record)]
pub struct RemoteArtworkFile {
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, uniffi::Record)]
pub struct HygraphAudioResponse {
    #[serde(rename = "xBibleAudioModules", default)]
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
#[serde(rename_all = "camelCase")]
pub struct RemoteAudioModuleInfo {
    #[serde(default)]
    pub id: Option<String>,
    pub unique_id: String,
    pub display_title: String,
    pub contributor: Option<String>,
    pub description: Option<String>,
    pub duration_ms: i64,
    #[serde(default)]
    pub features: HygraphFeatures,
    pub source_url: String,
    pub artwork_file: Option<RemoteArtworkFile>,

    // Hygraph returns `albums { id }`. Take the first element if present.
    // Also accept the singular camelCase form in case a future query changes.
    #[serde(
        rename = "albums",
        default,
        skip_serializing,
        deserialize_with = "de_first_or_single_id_ref"
    )]
    pub album_id: Option<IdRef>,

    pub version: i32,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub is_installed: bool,
    #[serde(skip)]
    pub status: ModuleStatus,

    // ── Artwork as bytes across the FFI boundary ──
    #[serde(skip)]
    pub artwork_bytes: Option<Vec<u8>>,
    #[serde(skip)]
    pub artwork_mime: Option<String>,

    #[serde(skip)]
    pub contributor_artwork_bytes: Option<Vec<u8>>,
    #[serde(skip)]
    pub contributor_artwork_mime: Option<String>,

    #[serde(skip)]
    pub album_artwork_bytes: Option<Vec<u8>>,
    #[serde(skip)]
    pub album_artwork_mime: Option<String>,
}

impl Default for RemoteAudioModuleInfo {
    fn default() -> Self {
        Self {
            id: None,
            unique_id: String::new(),
            display_title: String::new(),
            contributor: None,
            description: None,
            duration_ms: 0,
            features: HygraphFeatures::default(),
            source_url: String::new(),
            artwork_file: None,
            album_id: None,
            version: 1,
            created_at: String::new(),
            updated_at: String::new(),
            is_installed: false,
            status: ModuleStatus::Idle,
            artwork_bytes: None,
            artwork_mime: None,
            contributor_artwork_bytes: None,
            contributor_artwork_mime: None,
            album_artwork_bytes: None,
            album_artwork_mime: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct AudioLanguage {
    pub id: String,
    pub name: String,
    pub code: String,
    #[serde(default)]
    pub collections: Vec<AudioCollection>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct AudioAlbum {
    pub id: String,
    pub title: String,
    pub manifest_version: Option<String>,
    #[serde(default)]
    pub order: i32,

    // Hygraph query selects `collection { id }`.
    #[serde(rename = "collection", default)]
    pub collection_id: Option<IdRef>,

    // Hygraph query selects `xBibleAudioModules { ... }`.
    #[serde(rename = "xBibleAudioModules", default)]
    pub modules: Vec<RemoteAudioModuleInfo>,

    pub artwork: Option<RemoteArtworkFile>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct AudioCollection {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub manifest_version: Option<String>,
    #[serde(rename = "language", default)]
    pub language_id: Option<IdRef>,
    #[serde(default)]
    pub albums: Vec<AudioAlbum>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, uniffi::Record)]
#[serde(rename_all = "camelCase")]
pub struct AudioContributor {
    pub id: String,
    pub name: String,
    pub bio: Option<RichTextValue>,
    pub role: ContributorRole,
    pub photo: Option<RemoteArtworkFile>,
    #[serde(rename = "collections", default)]
    pub collections: Vec<IdRef>,
    pub created_at: String,
    pub updated_at: String,
}

impl AudioContributor {
    pub fn collection_ids(&self) -> impl Iterator<Item = &str> {
        self.collections.iter().map(|r| r.id.as_str())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, uniffi::Enum)]
#[serde(rename_all = "lowercase")]
pub enum ContributorRole {
    Narrator,
    Author,
    Producer,
    Translator,
    Editor,
}

// ─────────────────────────────────────────────────────────────────────────────
// GraphQL wrapper structs
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LanguagesData {
    #[serde(default)]
    languages: Vec<AudioLanguage>,
}

#[derive(Deserialize)]
struct CollectionsData {
    #[serde(default)]
    collections: Vec<AudioCollection>,
}

#[derive(Deserialize)]
struct AlbumsData {
    #[serde(default)]
    albums: Vec<AudioAlbum>,
}

#[derive(Deserialize)]
struct ContributorsData {
    #[serde(default)]
    contributors: Vec<AudioContributor>,
}

#[derive(Deserialize)]
struct ModulesData {
    #[serde(rename = "xBibleAudioModules", default)]
    modules: Vec<RemoteAudioModuleInfo>,
}

#[derive(Deserialize)]
struct LanguageSingle {
    language: Option<AudioLanguage>,
}

#[derive(Deserialize)]
struct CollectionSingle {
    collection: Option<AudioCollection>,
}

#[derive(Deserialize)]
struct AlbumSingle {
    album: Option<AudioAlbum>,
}

#[derive(Deserialize)]
struct ContributorSingle {
    contributor: Option<AudioContributor>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Query constants
// ─────────────────────────────────────────────────────────────────────────────

const Q_LANGUAGES: &str = r#"
query Languages($first: Int!, $skip: Int!) {
  languages(first: $first, skip: $skip, orderBy: code_ASC) {
    id name code createdAt updatedAt
  }
}
"#;

const Q_LANGUAGES_WITH_COLLECTIONS: &str = r#"
query LanguagesWithCollections($first: Int!, $skip: Int!) {
  languages(first: $first, skip: $skip, orderBy: code_ASC) {
    id name code createdAt updatedAt
    collections {
      id name description manifestVersion language { id }
      createdAt updatedAt
    }
  }
}
"#;

const Q_LANGUAGE_BY_ID: &str = r#"
query LanguageById($id: ID!) {
  language(where: { id: $id }) {
    id name code createdAt updatedAt
    collections {
      id name description manifestVersion language { id }
      createdAt updatedAt
    }
  }
}
"#;

const Q_COLLECTIONS: &str = r#"
query Collections($first: Int!, $skip: Int!) {
  collections(first: $first, skip: $skip, orderBy: name_ASC) {
    id name description manifestVersion language { id } createdAt updatedAt
  }
}
"#;

// Now selects album artwork.
const Q_COLLECTIONS_WITH_ALBUMS: &str = r#"
query CollectionsWithAlbums($first: Int!, $skip: Int!) {
  collections(first: $first, skip: $skip, orderBy: name_ASC) {
    id name description manifestVersion language { id } createdAt updatedAt
    albums(orderBy: order_ASC) {
      id title manifestVersion order collection { id }
      artwork { url }
      createdAt updatedAt
    }
  }
}
"#;

const Q_COLLECTIONS_BY_LANGUAGE: &str = r#"
query CollectionsByLanguage($code: String!, $first: Int!, $skip: Int!) {
  collections(
    where: { language: { code: $code } }
    first: $first, skip: $skip, orderBy: name_ASC
  ) {
    id name description manifestVersion language { id } createdAt updatedAt
  }
}
"#;

// Now selects album artwork + module artwork.
const Q_COLLECTION_BY_ID: &str = r#"
query CollectionById($id: ID!) {
  collection(where: { id: $id }) {
    id name description manifestVersion language { id } createdAt updatedAt
    albums(orderBy: order_ASC) {
      id title manifestVersion order collection { id }
      artwork { url }
      createdAt updatedAt
      xBibleAudioModules {
        id uniqueId displayTitle contributor description durationMs features
        sourceUrl artworkFile { url } version createdAt updatedAt
      }
    }
  }
}
"#;

// Now selects album artwork.
const Q_ALBUMS: &str = r#"
query Albums($first: Int!, $skip: Int!) {
  albums(first: $first, skip: $skip, orderBy: order_ASC) {
    id title manifestVersion order collection { id }
    artwork { url }
    createdAt updatedAt
  }
}
"#;

// Now selects album artwork.
const Q_ALBUMS_BY_COLLECTION: &str = r#"
query AlbumsByCollection($collectionId: ID!, $first: Int!, $skip: Int!) {
  albums(
    where: { collection: { id: $collectionId } }
    first: $first, skip: $skip, orderBy: order_ASC
  ) {
    id title manifestVersion order collection { id }
    artwork { url }
    createdAt updatedAt
  }
}
"#;

// Now selects album artwork + module artwork.
const Q_ALBUM_BY_ID: &str = r#"
query AlbumById($id: ID!) {
  album(where: { id: $id }) {
    id title manifestVersion order collection { id }
    artwork { url }
    createdAt updatedAt
    xBibleAudioModules {
      id uniqueId displayTitle contributor description durationMs features
      sourceUrl artworkFile { url } version createdAt updatedAt
    }
  }
}
"#;

const Q_MODULES: &str = r#"
query Modules($first: Int!, $skip: Int!) {
  xBibleAudioModules(first: $first, skip: $skip, orderBy: displayTitle_ASC) {
    id uniqueId displayTitle contributor description durationMs features
    sourceUrl artworkFile { url } version createdAt updatedAt
    albums { id }
  }
}
"#;

const Q_CONTRIBUTORS: &str = r#"
query Contributors($first: Int!, $skip: Int!) {
  contributors(first: $first, skip: $skip, orderBy: name_ASC) {
    id name bio { text } role photo { url } createdAt updatedAt
    collections { id }
  }
}
"#;

const Q_CONTRIBUTORS_BY_ROLE: &str = r#"
query ContributorsByRole($role: Rol!, $first: Int!, $skip: Int!) {
  contributors(
    where: { role: $role }
    first: $first, skip: $skip, orderBy: name_ASC
  ) {
    id name bio { text } role photo { url } createdAt updatedAt
    collections { id }
  }
}
"#;

const Q_CONTRIBUTOR_BY_ID: &str = r#"
query ContributorById($id: ID!) {
  contributor(where: { id: $id }) {
    id name bio { text } role photo { url } createdAt updatedAt
    collections { id }
  }
}
"#;

const Q_NARRATORS_FOR_COLLECTION: &str = r#"
query NarratorsForCollection($collectionId: ID!) {
  contributors(
    where: { role: Narrator, collections_some: { id: $collectionId } }
    first: 10
    orderBy: name_ASC
  ) {
    id name bio { text } role photo { url } createdAt updatedAt
    collections { id }
  }
}
"#;

// ─────────────────────────────────────────────────────────────────────────────
// Client
// ─────────────────────────────────────────────────────────────────────────────

#[derive(uniffi::Object)]
pub struct StoreApiClient {
    client: Client,
    endpoint_url: String,
    auth_token: Option<String>,
}

#[derive(Debug, Clone, thiserror::Error, uniffi::Error)]
pub enum StoreApiError {
    #[error("Network infrastructure failure: {error_message}")]
    NetworkFailure { error_message: String },
    #[error("API Deserialization payload error: {error_message}")]
    SerializationFailure { error_message: String },
    #[error("Local filesystem file I/O layout drop: {error_message}")]
    IoFailure { error_message: String },
    #[error("GraphQL query failed: {error_message}")]
    QueryFailure { error_message: String },
    #[error("Record not found: {id}")]
    NotFound { id: String },
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
        handler: Arc<dyn RustDownloadProgressHandler>,
    ) -> Self {
        Self {
            unique_id_filter,
            rust_handler: Some(handler),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GraphQL helper
// ─────────────────────────────────────────────────────────────────────────────

impl StoreApiClient {
    async fn graphql<T>(
        &self,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<T, StoreApiError>
    where
        T: serde::de::DeserializeOwned,
    {
        let payload = serde_json::json!({ "query": query, "variables": variables });
        let mut request = self.client.post(&self.endpoint_url).json(&payload);
        if let Some(ref token) = self.auth_token {
            request = request.header("Authorization", format!("Bearer {}", token));
        }
        let response = request
            .send()
            .await
            .map_err(|e| StoreApiError::NetworkFailure {
                error_message: e.to_string(),
            })?;
        let status = response.status();
        let value: serde_json::Value =
            response
                .json()
                .await
                .map_err(|e| StoreApiError::SerializationFailure {
                    error_message: e.to_string(),
                })?;
        if !status.is_success() {
            return Err(StoreApiError::NetworkFailure {
                error_message: format!("HTTP {}: {}", status, value),
            });
        }
        if let Some(errors) = value.get("errors") {
            return Err(StoreApiError::QueryFailure {
                error_message: errors.to_string(),
            });
        }
        let data = value
            .get("data")
            .ok_or_else(|| StoreApiError::SerializationFailure {
                error_message: format!("No 'data' field in response: {}", value),
            })?;
        serde_json::from_value(data.clone()).map_err(|e| StoreApiError::SerializationFailure {
            error_message: e.to_string(),
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API
// ─────────────────────────────────────────────────────────────────────────────

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
            .timeout(Duration::from_secs(600))
            .read_timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(15))
            .tcp_keepalive(Duration::from_secs(15))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            client,
            endpoint_url,
            auth_token,
        }
    }
    // ── Languages ──────────────────────────────────────────────────────────

    pub async fn fetch_languages(
        &self,
        first: i32,
        skip: i32,
    ) -> Result<Vec<AudioLanguage>, StoreApiError> {
        let (first, skip) = clamp_page(first, skip);
        let d: LanguagesData = self
            .graphql(
                Q_LANGUAGES,
                serde_json::json!({ "first": first, "skip": skip }),
            )
            .await?;
        Ok(d.languages)
    }

    pub async fn fetch_languages_with_collections(
        &self,
        first: i32,
        skip: i32,
    ) -> Result<Vec<AudioLanguage>, StoreApiError> {
        let (first, skip) = clamp_page(first, skip);
        let d: LanguagesData = self
            .graphql(
                Q_LANGUAGES_WITH_COLLECTIONS,
                serde_json::json!({ "first": first, "skip": skip }),
            )
            .await?;
        Ok(d.languages)
    }

    pub async fn fetch_language(&self, id: String) -> Result<AudioLanguage, StoreApiError> {
        let d: LanguageSingle = self
            .graphql(Q_LANGUAGE_BY_ID, serde_json::json!({ "id": id }))
            .await?;
        d.language.ok_or(StoreApiError::NotFound { id })
    }

    // ── Collections ────────────────────────────────────────────────────────

    pub async fn fetch_collections(
        &self,
        first: i32,
        skip: i32,
    ) -> Result<Vec<AudioCollection>, StoreApiError> {
        let (first, skip) = clamp_page(first, skip);
        let d: CollectionsData = self
            .graphql(
                Q_COLLECTIONS,
                serde_json::json!({ "first": first, "skip": skip }),
            )
            .await?;
        Ok(d.collections)
    }

    pub async fn fetch_collections_with_albums(
        &self,
        first: i32,
        skip: i32,
    ) -> Result<Vec<AudioCollection>, StoreApiError> {
        let (first, skip) = clamp_page(first, skip);
        let d: CollectionsData = self
            .graphql(
                Q_COLLECTIONS_WITH_ALBUMS,
                serde_json::json!({ "first": first, "skip": skip }),
            )
            .await?;
        Ok(d.collections)
    }

    pub async fn fetch_collections_by_language(
        &self,
        code: String,
        first: i32,
        skip: i32,
    ) -> Result<Vec<AudioCollection>, StoreApiError> {
        let (first, skip) = clamp_page(first, skip);
        let d: CollectionsData = self
            .graphql(
                Q_COLLECTIONS_BY_LANGUAGE,
                serde_json::json!({ "code": code, "first": first, "skip": skip }),
            )
            .await?;
        Ok(d.collections)
    }

    pub async fn fetch_collection(&self, id: String) -> Result<AudioCollection, StoreApiError> {
        let d: CollectionSingle = self
            .graphql(Q_COLLECTION_BY_ID, serde_json::json!({ "id": id }))
            .await?;
        d.collection.ok_or(StoreApiError::NotFound { id })
    }

    // ── Albums ─────────────────────────────────────────────────────────────

    pub async fn fetch_albums(
        &self,
        first: i32,
        skip: i32,
    ) -> Result<Vec<AudioAlbum>, StoreApiError> {
        let (first, skip) = clamp_page(first, skip);
        let d: AlbumsData = self
            .graphql(
                Q_ALBUMS,
                serde_json::json!({ "first": first, "skip": skip }),
            )
            .await?;
        Ok(d.albums)
    }

    pub async fn fetch_albums_by_collection(
        &self,
        collection_id: String,
        first: i32,
        skip: i32,
    ) -> Result<Vec<AudioAlbum>, StoreApiError> {
        let (first, skip) = clamp_page(first, skip);
        let d: AlbumsData = self
            .graphql(
                Q_ALBUMS_BY_COLLECTION,
                serde_json::json!({ "collectionId": collection_id, "first": first, "skip": skip }),
            )
            .await?;
        Ok(d.albums)
    }

    pub async fn fetch_album(&self, id: String) -> Result<AudioAlbum, StoreApiError> {
        let d: AlbumSingle = self
            .graphql(Q_ALBUM_BY_ID, serde_json::json!({ "id": id }))
            .await?;
        d.album.ok_or(StoreApiError::NotFound { id })
    }

    // ── Audio modules (download catalog) ───────────────────────────────────

    pub async fn fetch_audio_modules(
        &self,
        first: i32,
        skip: i32,
    ) -> Result<Vec<RemoteAudioModuleInfo>, StoreApiError> {
        let (first, skip) = clamp_page(first, skip);
        let d: ModulesData = self
            .graphql(
                Q_MODULES,
                serde_json::json!({ "first": first, "skip": skip }),
            )
            .await?;
        Ok(d.modules)
    }

    // ── Contributors ───────────────────────────────────────────────────────

    pub async fn fetch_contributors(
        &self,
        first: i32,
        skip: i32,
    ) -> Result<Vec<AudioContributor>, StoreApiError> {
        let (first, skip) = clamp_page(first, skip);
        let d: ContributorsData = self
            .graphql(
                Q_CONTRIBUTORS,
                serde_json::json!({ "first": first, "skip": skip }),
            )
            .await?;
        Ok(d.contributors)
    }

    pub async fn fetch_contributors_by_role(
        &self,
        role: ContributorRole,
        first: i32,
        skip: i32,
    ) -> Result<Vec<AudioContributor>, StoreApiError> {
        let (first, skip) = clamp_page(first, skip);
        let d: ContributorsData = self
            .graphql(
                Q_CONTRIBUTORS_BY_ROLE,
                serde_json::json!({ "role": role, "first": first, "skip": skip }),
            )
            .await?;
        Ok(d.contributors)
    }

    pub async fn fetch_contributor(&self, id: String) -> Result<AudioContributor, StoreApiError> {
        let d: ContributorSingle = self
            .graphql(Q_CONTRIBUTOR_BY_ID, serde_json::json!({ "id": id }))
            .await?;
        d.contributor.ok_or(StoreApiError::NotFound { id })
    }

    pub async fn fetch_narrators_for_collection(
        &self,
        collection_id: String,
    ) -> Result<Vec<AudioContributor>, StoreApiError> {
        let d: ContributorsData = self
            .graphql(
                Q_NARRATORS_FOR_COLLECTION,
                serde_json::json!({ "collectionId": collection_id }),
            )
            .await?;
        Ok(d.contributors)
    }

    pub async fn download_and_install_module(
        &self,
        module: RemoteAudioModuleInfo,
        modules_root_path: String,
        location: ModuleLocation,
        collection: AudioCollection,
        album: AudioAlbum,
        contributor: Option<AudioContributor>,
        progress_listener: Arc<StoreDownloadProgressListener>,
    ) -> Result<String, StoreApiError> {
        use crate::engines::audio_engine::store::local_store::module_path;

        let modules_root = Path::new(&modules_root_path);
        let destination_path = module_path(modules_root, &location);
        let temp_path = destination_path.with_extension("xba.download");

        if let Some(parent) = destination_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| StoreApiError::IoFailure {
                    error_message: format!("Failed to create hierarchy folders: {}", e),
                })?;
        }

        let cleanup = |path: &Path| {
            let _ = std::fs::remove_file(path);
        };

        // ── 1. Resume state ──
        let mut initial_bytes: u64 = 0;
        if temp_path.exists() {
            if let Ok(meta) = std::fs::metadata(&temp_path) {
                initial_bytes = meta.len();
            }
        }

        let mut req_builder = self
            .client
            .get(&module.source_url)
            .header(reqwest::header::ACCEPT_ENCODING, "identity");

        if initial_bytes > 0 {
            req_builder =
                req_builder.header(reqwest::header::RANGE, format!("bytes={}-", initial_bytes));
        }

        let response = match req_builder.send().await {
            Ok(r) => r,
            Err(e) => {
                cleanup(&temp_path);
                return Err(StoreApiError::NetworkFailure {
                    error_message: format!("Handshake initialization failure: {}", e),
                });
            }
        };

        let status = response.status();

        let (mut bytes_written, total_size) = if status == reqwest::StatusCode::PARTIAL_CONTENT {
            let remaining = response.content_length().unwrap_or(0);
            (initial_bytes, Some(initial_bytes + remaining))
        } else if status.is_success() {
            if initial_bytes > 0 {
                let _ = tokio::fs::remove_file(&temp_path).await;
            }
            (0, response.content_length())
        } else {
            cleanup(&temp_path);
            return Err(StoreApiError::NetworkFailure {
                error_message: format!("HTTP {} from source_url", status),
            });
        };

        // ── 2. Already complete? ──
        if let Some(total) = total_size {
            if initial_bytes >= total && total > 0 {
                if let Err(e) = tokio::fs::rename(&temp_path, &destination_path).await {
                    cleanup(&temp_path);
                    return Err(StoreApiError::IoFailure {
                        error_message: format!("Failed to finalize complete file: {}", e),
                    });
                }
                progress_listener.on_progress(module.unique_id.clone(), total, Some(total));
                return Ok(destination_path.to_string_lossy().into_owned());
            }
        }

        // ── 3. Open temp ──
        let file = match tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .append(bytes_written > 0)
            .open(&temp_path)
            .await
        {
            Ok(f) => f,
            Err(e) => {
                cleanup(&temp_path);
                return Err(StoreApiError::IoFailure {
                    error_message: format!("Failed to open temp destination: {}", e),
                });
            }
        };

        let chunk_threshold = match total_size {
            Some(total) if total > 200 => total / 200,
            _ => 64 * 1024,
        };

        // ── 4. Stream body ──
        let (tx, mut rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(16);
        let mut stream_response = response;

        tokio::spawn(async move {
            while let Ok(Some(chunk)) = stream_response.chunk().await {
                if tx.send(chunk).await.is_err() {
                    break;
                }
            }
        });

        let mut buffered_writer = BufWriter::with_capacity(256 * 1024, file);
        let mut last_reported_bytes = bytes_written;
        let mut last_update_time = Instant::now();
        let throttle_interval = Duration::from_millis(250);

        progress_listener.on_progress(module.unique_id.clone(), bytes_written, total_size);

        while let Some(chunk) = rx.recv().await {
            if let Err(e) = buffered_writer.write_all(&chunk).await {
                drop(buffered_writer);
                cleanup(&temp_path);
                return Err(StoreApiError::IoFailure {
                    error_message: format!("Disk write failure: {}", e),
                });
            }
            bytes_written += chunk.len() as u64;

            let bytes_since_report = bytes_written.saturating_sub(last_reported_bytes);
            let time_since_report = last_update_time.elapsed();

            if bytes_since_report >= chunk_threshold || time_since_report >= throttle_interval {
                let listener_clone = Arc::clone(&progress_listener);
                let id_clone = module.unique_id.clone();
                tokio::spawn(async move {
                    listener_clone.on_progress(id_clone, bytes_written, total_size);
                });
                last_reported_bytes = bytes_written;
                last_update_time = Instant::now();
            }
        }

        if let Err(e) = buffered_writer.flush().await {
            drop(buffered_writer);
            cleanup(&temp_path);
            return Err(StoreApiError::IoFailure {
                error_message: format!("Failed to flush temp file: {}", e),
            });
        }
        drop(buffered_writer);

        // ── 5. Verify completeness ──
        if let Some(total) = total_size {
            if bytes_written < total {
                cleanup(&temp_path);
                return Err(StoreApiError::NetworkFailure {
                    error_message: format!(
                        "Incomplete download: {} of {} bytes",
                        bytes_written, total
                    ),
                });
            }
        } else {
            if let Ok(meta) = std::fs::metadata(&temp_path) {
                bytes_written = meta.len();
            }
            if bytes_written == 0 {
                cleanup(&temp_path);
                return Err(StoreApiError::NetworkFailure {
                    error_message: "Zero-byte download".to_string(),
                });
            }
        }

        // ── 6. Stamp (blocking, uses blocking::Client for artwork) ──
        let stamp_path = temp_path.clone();
        let location_clone = location.clone();
        let collection_clone = collection.clone();
        let album_clone = album.clone();
        let module_source_url = module.source_url.clone();
        let contributor_photo_url = contributor
            .as_ref()
            .and_then(|c| c.photo.as_ref().map(|p| p.url.clone()));
        let album_artwork_url = album.artwork.as_ref().map(|a| a.url.clone());

        let stamp_result: Result<(), String> = tokio::task::spawn_blocking(move || {
            // 6a. Read stub
            let mut stub =
                read_stub_metadata(&stamp_path).map_err(|e| format!("Stub read failed: {}", e))?;

            // The .xba file's internal metadata was authored by whoever
            // uploaded the blob, and may not match Hygraph's identity for
            // this module. Hygraph is authoritative — overwrite the
            // identity fields so the stamped file reflects what the
            // catalog says. This is why legacy uploads whose stub says
            // e.g. "eng-kjv-1john" end up correctly stamped as "1john_kjv".
            stub.unique_id = location_clone.module_unique_id.clone();
            stub.version = location_clone.module_version;
            stub.source_url = module_source_url.clone();

            // 6b. Fetch artwork synchronously inside the blocking task.
            //     Uses its own reqwest::blocking client — no .await needed.
            let mut blocking_headers = reqwest::header::HeaderMap::new();
            blocking_headers.insert(
                reqwest::header::USER_AGENT,
                reqwest::header::HeaderValue::from_static("X-Bible-Engine/1.0"),
            );
            let blocking_client = reqwest::blocking::Client::builder()
                .default_headers(blocking_headers)
                .timeout(std::time::Duration::from_secs(30))
                .connect_timeout(std::time::Duration::from_secs(15))
                .build()
                .ok();

            let fetch_art = |url: &str| -> Option<(Vec<u8>, String)> {
                let client = blocking_client.as_ref()?;
                let resp = client.get(url).send().ok()?;
                if !resp.status().is_success() {
                    return None;
                }
                let ct = resp
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("image/jpeg")
                    .to_string();
                let ext = match ct.as_str() {
                    "image/png" => "png",
                    "image/webp" => "webp",
                    "image/gif" => "gif",
                    _ => "jpg",
                };
                let bytes = resp.bytes().ok()?.to_vec();
                if bytes.is_empty() {
                    return None;
                }
                Some((bytes, ext.to_string()))
            };

            let contributor_art = contributor_photo_url
                .as_deref()
                .and_then(fetch_art)
                .map(|(bytes, ext)| (format!("artwork_contributor.{}", ext), bytes));
            let album_art = album_artwork_url
                .as_deref()
                .and_then(fetch_art)
                .map(|(bytes, ext)| (format!("artwork_album.{}", ext), bytes));

            // 6c. Build stamped metadata
            let mut stamped =
                XbaMetadata::stamp(stub, &location_clone, &collection_clone, &album_clone);
            if let Some((name, _)) = contributor_art.as_ref() {
                stamped.organization.contributor_artwork_entry = Some(name.clone());
            }
            if let Some((name, _)) = album_art.as_ref() {
                stamped.organization.album_artwork_entry = Some(name.clone());
            }

            // 6d. Rewrite
            rewrite_xba_with_stamp(
                &stamp_path,
                &stamped,
                contributor_art
                    .as_ref()
                    .map(|(n, b)| (n.as_str(), b.as_slice())),
                album_art.as_ref().map(|(n, b)| (n.as_str(), b.as_slice())),
            )
            .map_err(|e| format!("Rewrite failed: {}", e))?;

            // 6e. Verify. Note: we just *set* stub.unique_id/version/source_url
            //     before the rewrite, so this is checking that our own write
            //     persisted — not that the file matches Hygraph. If this
            //     fails, the rewrite step is broken.
            let check = read_stamped_metadata(&stamp_path)
                .map_err(|e| format!("Verification read failed: {}", e))?;
            if check.organization.stamped_at.is_empty() {
                return Err("Verification failed: no stamped_at".to_string());
            }
            if check.stub.unique_id != location_clone.module_unique_id {
                return Err(format!(
                    "Verification failed: unique_id mismatch after rewrite ({} vs {})",
                    check.stub.unique_id, location_clone.module_unique_id
                ));
            }
            if check.stub.version != location_clone.module_version {
                return Err(format!(
                    "Verification failed: version mismatch after rewrite ({} vs {})",
                    check.stub.version, location_clone.module_version
                ));
            }

            Ok(())
        })
        .await
        .unwrap_or_else(|e| Err(format!("Stamp task panicked: {}", e)));

        if let Err(e) = stamp_result {
            cleanup(&temp_path);
            return Err(StoreApiError::SerializationFailure { error_message: e });
        }

        // ── 7. Atomic rename ──
        if let Err(e) = tokio::fs::rename(&temp_path, &destination_path).await {
            cleanup(&temp_path);
            return Err(StoreApiError::IoFailure {
                error_message: format!("Failed to finalize xba into place: {}", e),
            });
        }

        progress_listener.on_progress(module.unique_id.clone(), bytes_written, total_size);
        Ok(destination_path.to_string_lossy().into_owned())
    }
}

/// Deserialize Hygraph's `albums: [{ id }]` (list) into `Option<IdRef>`,
/// taking the first element. Also accepts a single object (for queries
/// that select `album { id }`) and `null`.
fn de_first_or_single_id_ref<'de, D>(d: D) -> Result<Option<IdRef>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        Many(Vec<IdRef>),
        One(IdRef),
        None,
    }

    match OneOrMany::deserialize(d)? {
        OneOrMany::Many(v) => Ok(v.into_iter().next()),
        OneOrMany::One(r) => Ok(Some(r)),
        OneOrMany::None => Ok(None),
    }
}
