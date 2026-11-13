// src/engines/audio_engine/audio_engine.rs

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};

use chacha20poly1305::{
    ChaCha20Poly1305, Nonce,
    aead::{Aead, KeyInit},
};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::engines::audio_engine::store::local_store::{
    XbaMetadata, read_entry_bytes, read_stamped_metadata,
};
use crate::engines::audio_engine::utils::artwork::Artwork;

const SECRET_KEY: &[u8; 32] = &[
    0x5f, 0x93, 0xbc, 0x1d, 0x07, 0x58, 0x1a, 0x82, 0x4b, 0x15, 0x26, 0x0c, 0x9a, 0xec, 0x95, 0x3c,
    0x87, 0x60, 0x9e, 0x62, 0x12, 0x28, 0x9d, 0xfa, 0xcd, 0x3e, 0x7b, 0x03, 0xef, 0x81, 0x44, 0xd2,
];

// ─────────────────────────────────────────────────────────────────────────────
// Errors
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum AudioEngineError {
    #[error("Container package item not found: {path}")]
    ModuleNotFound { path: String },
    #[error("File system I/O error occurred: {error_message}")]
    IoFailure { error_message: String },
    #[error("Failed to parse structural JSON content format: {error_message}")]
    SerializationFailure { error_message: String },
    #[error("Decryption safety failure: {error_message}")]
    DecryptionFailure { error_message: String },
}

impl From<std::io::Error> for AudioEngineError {
    fn from(err: std::io::Error) -> Self {
        AudioEngineError::IoFailure {
            error_message: err.to_string(),
        }
    }
}

impl From<serde_json::Error> for AudioEngineError {
    fn from(err: serde_json::Error) -> Self {
        AudioEngineError::SerializationFailure {
            error_message: err.to_string(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Enums and node types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RepeatMode {
    Off,
    One,
    All,
}

#[derive(Debug, Clone, uniffi::Record, serde::Deserialize)]
pub struct AudioNode {
    pub r#type: String,
    pub id: String,
    pub title: String,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub text: Option<String>,
    #[serde(default)]
    pub children: Vec<AudioNode>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Metadata — carries the stamped organization block from the archive
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, uniffi::Record, Serialize, Deserialize)]
pub struct ModuleOrganization {
    pub language_code: String,
    pub language_name: String,
    pub contributor_name: Option<String>,
    pub collection_id: String,
    pub collection_name: String,
    pub collection_description: Option<String>,
    pub album_id: String,
    pub album_title: String,
    pub album_manifest_version: Option<String>,
    pub album_order: i32,
    pub stamped_at: String,
    pub stamped_by_version: String,
}

#[derive(Debug, Clone, uniffi::Record, Serialize, Deserialize)]
pub struct ModuleMetadata {
    pub unique_id: String,
    pub display_title: String,
    pub version: i32,
    pub language: String,
    pub contributor: String,
    pub description: String,
    pub source_url: String,
    pub duration_ms: i64,
    pub features: Vec<String>,
    pub artwork_file: Option<String>,
    pub organization: Option<ModuleOrganization>,
}

impl ModuleMetadata {
    /// Build from the stamped archive metadata. Every stub field is preserved;
    /// the org block is lifted into its own record.
    fn from_stamped(meta: &XbaMetadata) -> Self {
        Self {
            unique_id: meta.stub.unique_id.clone(),
            display_title: meta.stub.display_title.clone(),
            version: meta.stub.version,
            language: meta.stub.language.clone(),
            contributor: meta.stub.contributor.clone(),
            description: meta.stub.description.clone(),
            source_url: meta.stub.source_url.clone(),
            duration_ms: meta.stub.duration_ms,
            features: meta.stub.features.clone(),
            artwork_file: meta.stub.artwork_file.clone(),
            organization: Some(ModuleOrganization {
                language_code: meta.organization.language_code.clone(),
                language_name: meta.organization.language_name.clone(),
                contributor_name: meta.organization.contributor_name.clone(),
                collection_id: meta.organization.collection_id.clone(),
                collection_name: meta.organization.collection_name.clone(),
                collection_description: meta.organization.collection_description.clone(),
                album_id: meta.organization.album_id.clone(),
                album_title: meta.organization.album_title.clone(),
                album_manifest_version: meta.organization.album_manifest_version.clone(),
                album_order: meta.organization.album_order,
                stamped_at: meta.organization.stamped_at.clone(),
                stamped_by_version: meta.organization.stamped_by_version.clone(),
            }),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Installed module view for the UI
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, uniffi::Record)]
pub struct AudioModuleInfo {
    pub file_name: String,
    pub absolute_path: String,
    pub metadata: Option<ModuleMetadata>,
    pub artwork: Arc<Artwork>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct PlaybackState {
    pub current_time_ms: i64,
    pub active_anchor_index: i32,
    pub active_text: String,
    pub is_playing: bool,
    pub repeat_mode: RepeatMode,
}

#[derive(Debug, Clone, uniffi::Record)]
struct InternalPlaybackState {
    current_time_ms: i64,
    is_playing: bool,
    repeat_mode: RepeatMode,
    current_duration_ms: i64,
}

// ─────────────────────────────────────────────────────────────────────────────
// AudioEngine
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, uniffi::Object)]
pub struct AudioEngine {
    current_tree: Mutex<Option<AudioNode>>,
    loaded_audio_bytes: Mutex<Option<Vec<u8>>>,
    playback_state: Mutex<InternalPlaybackState>,
}

#[uniffi::export]
impl AudioEngine {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(AudioEngine {
            current_tree: Mutex::new(None),
            loaded_audio_bytes: Mutex::new(None),
            playback_state: Mutex::new(InternalPlaybackState {
                current_time_ms: 0,
                is_playing: false,
                repeat_mode: RepeatMode::Off,
                current_duration_ms: 0,
            }),
        })
    }

    pub fn get_audio_modules_path(&self) -> String {
        let proj_dirs = ProjectDirs::from("org", "flame", "xbible").expect("Path error");
        let path = proj_dirs.data_local_dir().join("modules").join("audio");
        fs::create_dir_all(&path).ok();
        path.to_string_lossy().into_owned()
    }

    // ── Module discovery (hierarchy-aware) ────────────────────────────────

    /// Walk the hierarchy root and return every stamped module.
    /// Unstamped archives are skipped — only app-blessed modules are visible.
    pub fn get_audio_modules(&self) -> Vec<AudioModuleInfo> {
        let root_str = self.get_audio_modules_path();
        let root = Path::new(&root_str);
        let mut modules = Vec::new();

        for entry in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let file_path = entry.path();
            if !file_path.is_file() {
                continue;
            }
            if !file_path
                .extension()
                .map_or(false, |ext| ext.eq_ignore_ascii_case("xba"))
            {
                continue;
            }

            let Ok(stamped) = read_stamped_metadata(file_path) else {
                continue;
            };
            if stamped.organization.stamped_at.is_empty() {
                continue; // not stamped by us
            }

            let metadata = Some(ModuleMetadata::from_stamped(&stamped));
            let artwork = Artwork::new(file_path.to_string_lossy().into_owned());

            if let Some(name_str) = file_path.file_name().and_then(|n| n.to_str()) {
                modules.push(AudioModuleInfo {
                    file_name: name_str.to_string(),
                    absolute_path: file_path.to_string_lossy().into_owned(),
                    metadata,
                    artwork: Arc::new(artwork),
                });
            }
        }

        modules
    }

    /// Read a single module's stamped metadata without loading audio.
    pub fn peek_module_metadata(
        &self,
        file_path: String,
    ) -> Result<ModuleMetadata, AudioEngineError> {
        let path = Path::new(&file_path);
        if !path.exists() {
            return Err(AudioEngineError::ModuleNotFound { path: file_path });
        }
        let stamped =
            read_stamped_metadata(path).map_err(|e| AudioEngineError::SerializationFailure {
                error_message: e.to_string(),
            })?;
        if stamped.organization.stamped_at.is_empty() {
            return Err(AudioEngineError::SerializationFailure {
                error_message: "Archive has not been stamped by the app".to_string(),
            });
        }
        Ok(ModuleMetadata::from_stamped(&stamped))
    }

    /// Read artwork bytes directly out of the archive.
    /// `kind` is "server" | "contributor" | "album".
    pub fn get_module_artwork(
        &self,
        file_path: String,
        kind: String,
    ) -> Result<Vec<u8>, AudioEngineError> {
        let path = Path::new(&file_path);
        if !path.exists() {
            return Err(AudioEngineError::ModuleNotFound { path: file_path });
        }
        let stamped =
            read_stamped_metadata(path).map_err(|e| AudioEngineError::SerializationFailure {
                error_message: e.to_string(),
            })?;

        let entry = match kind.as_str() {
            "server" => stamped.stub.artwork_file.clone(),
            "contributor" => stamped.organization.contributor_artwork_entry.clone(),
            "album" => stamped.organization.album_artwork_entry.clone(),
            _ => None,
        };

        let Some(name) = entry else {
            return Ok(Vec::new()); // absent artwork → empty bytes, not an error
        };
        read_entry_bytes(path, &name).map_err(|e| AudioEngineError::IoFailure {
            error_message: e.to_string(),
        })
    }

    // ── Audio loading (unchanged logic) ───────────────────────────────────

    pub fn load_audio_module(&self, file_path: String) -> Result<Vec<u8>, AudioEngineError> {
        let path = Path::new(&file_path);
        if !path.exists() {
            return Err(AudioEngineError::ModuleNotFound { path: file_path });
        }

        let file = File::open(path)?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| AudioEngineError::IoFailure {
            error_message: format!("Invalid archive structure: {}", e),
        })?;

        let duration = read_stamped_metadata(path)
            .ok()
            .map(|m| m.stub.duration_ms)
            .unwrap_or(0);

        let root_tree: AudioNode =
            if let Ok(mut timestamps_file) = archive.by_name("timestamps.json") {
                let mut json_contents = String::new();
                timestamps_file.read_to_string(&mut json_contents)?;
                serde_json::from_str(&json_contents)?
            } else {
                return Err(AudioEngineError::IoFailure {
                    error_message: "Missing required timestamps.json file".to_string(),
                });
            };

        let encrypted_audio_bytes = if let Ok(mut audio_file) = archive.by_name("audio.mp3") {
            let mut bytes = Vec::new();
            audio_file.read_to_end(&mut bytes)?;
            bytes
        } else {
            return Err(AudioEngineError::IoFailure {
                error_message: "Missing required audio.mp3 file".to_string(),
            });
        };

        let cipher = ChaCha20Poly1305::new(SECRET_KEY.into());
        let nonce = Nonce::from_slice(b"xbible_media");
        let decrypted_audio_bytes = cipher
            .decrypt(nonce, encrypted_audio_bytes.as_slice())
            .map_err(|e| AudioEngineError::DecryptionFailure {
                error_message: e.to_string(),
            })?;

        *self.current_tree.lock().unwrap() = Some(root_tree);
        *self.loaded_audio_bytes.lock().unwrap() = Some(decrypted_audio_bytes.clone());

        if let Ok(mut state) = self.playback_state.lock() {
            state.current_time_ms = 0;
            state.is_playing = false;
            state.current_duration_ms = duration;
        }

        Ok(decrypted_audio_bytes)
    }

    // ── Transport (unchanged) ─────────────────────────────────────────────

    pub fn stop(&self) {
        if let Ok(mut state) = self.playback_state.lock() {
            state.is_playing = false;
            state.current_time_ms = 0;
        }
    }

    pub fn skip_forward(&self, seconds: i64) {
        if let Ok(mut state) = self.playback_state.lock() {
            let limit = state.current_duration_ms;
            state.current_time_ms = (state.current_time_ms + (seconds * 1000)).min(limit);
        }
    }

    pub fn skip_backward(&self, seconds: i64) {
        if let Ok(mut state) = self.playback_state.lock() {
            state.current_time_ms = (state.current_time_ms - (seconds * 1000)).max(0);
        }
    }

    pub fn set_repeat_mode(&self, mode: RepeatMode) {
        if let Ok(mut state) = self.playback_state.lock() {
            state.repeat_mode = mode;
        }
    }

    pub fn toggle_playback(&self) {
        if let Ok(mut state) = self.playback_state.lock() {
            state.is_playing = !state.is_playing;
        }
    }

    pub fn seek_to_time(&self, target_ms: i64) {
        if let Ok(mut state) = self.playback_state.lock() {
            let limit = state.current_duration_ms;
            state.current_time_ms = target_ms.clamp(0, limit);
        }
    }

    pub fn seek_to_chapter(&self, chapter_id: String) {
        let tree_lock = self.current_tree.lock().unwrap();
        if let Some(ref root) = *tree_lock {
            if let Some(node) = find_node_by_id(root, &chapter_id) {
                if let Some(start) = node.start_ms {
                    self.seek_to_time(start);
                }
            }
        }
    }

    pub fn get_playback_state(&self) -> Option<PlaybackState> {
        let state = self.playback_state.lock().ok()?;
        let tree_lock = self.current_tree.lock().ok()?;
        let root_node = tree_lock.as_ref()?;

        let mut active_text = String::new();
        let mut active_index = -1;

        if let Some(matching_verse) = find_active_verse_leaf(root_node, state.current_time_ms) {
            active_text = matching_verse.text.clone().unwrap_or_default();
            active_index = 1;
        }

        Some(PlaybackState {
            current_time_ms: state.current_time_ms,
            active_anchor_index: active_index,
            active_text,
            is_playing: state.is_playing,
            repeat_mode: state.repeat_mode,
        })
    }

    pub fn get_navigation_tree(&self) -> Option<AudioNode> {
        self.current_tree.lock().unwrap().clone()
    }

    pub fn find_active_node_id(&self, time_ms: i64) -> Option<String> {
        let root = self.get_navigation_tree()?;
        find_active_verse_leaf(&root, time_ms).map(|n| n.id.clone())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tree helpers (unchanged)
// ─────────────────────────────────────────────────────────────────────────────

pub fn find_node_by_id<'a>(node: &'a AudioNode, id: &str) -> Option<&'a AudioNode> {
    if node.id == id {
        return Some(node);
    }
    for child in &node.children {
        if let Some(found) = find_node_by_id(child, id) {
            return Some(found);
        }
    }
    None
}

pub fn find_active_verse_leaf<'a>(node: &'a AudioNode, time_ms: i64) -> Option<&'a AudioNode> {
    if let (Some(start), Some(end)) = (node.start_ms, node.end_ms) {
        if time_ms >= start && time_ms <= end {
            if node.children.is_empty() {
                return Some(node);
            }
            for child in &node.children {
                if let Some(found) = find_active_verse_leaf(child, time_ms) {
                    return Some(found);
                }
            }
            return Some(node);
        }
    } else {
        for child in &node.children {
            if let Some(found) = find_active_verse_leaf(child, time_ms) {
                return Some(found);
            }
        }
    }
    None
}
