use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};

use chacha20poly1305::{
    ChaCha20Poly1305, Nonce,
    aead::{Aead, KeyInit},
};
use directories::ProjectDirs;

use crate::engines::audio_engine::utils::artwork::Artwork;

const SECRET_KEY: &[u8; 32] = &[
    0x5f, 0x93, 0xbc, 0x1d, 0x07, 0x58, 0x1a, 0x82, 0x4b, 0x15, 0x26, 0x0c, 0x9a, 0xec, 0x95, 0x3c,
    0x87, 0x60, 0x9e, 0x62, 0x12, 0x28, 0x9d, 0xfa, 0xcd, 0x3e, 0x7b, 0x03, 0xef, 0x81, 0x44, 0xd2,
];

// --- ERROR HANDLING ---

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum AudioEngineError {
    #[error("Container package item not found: {path}")]
    ModuleNotFound { path: String },
    #[error("File system I/O error occurred: {message}")]
    IoFailure { message: String },
    #[error("Failed to parse structural JSON content format: {message}")]
    SerializationFailure { message: String },
    #[error("Decryption safety failure: {message}")]
    DecryptionFailure { message: String },
}

impl From<std::io::Error> for AudioEngineError {
    fn from(err: std::io::Error) -> Self {
        AudioEngineError::IoFailure { message: err.to_string() }
    }
}

impl From<serde_json::Error> for AudioEngineError {
    fn from(err: serde_json::Error) -> Self {
        AudioEngineError::SerializationFailure { message: err.to_string() }
    }
}

// --- NEW ENUM: REPEAT LAYER ---

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RepeatMode {
    Off,
    One,
    All,
}

// --- DATA STRUCTURE RECORDS ---

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

#[derive(Debug, Clone, uniffi::Record, serde::Deserialize)]
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
}

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
    pub repeat_mode: RepeatMode, // 🌟 EXPOSED STATE
}

// --- INTERNAL PLAYBACK MUTABLE STATE MACHINE ---

struct InternalPlaybackState {
    current_time_ms: i64,
    is_playing: bool,
    repeat_mode: RepeatMode,
    current_duration_ms: i64,
}

// --- AUDIO ENGINE COORDINATOR ---

#[derive(uniffi::Object)]
pub struct AudioEngine {
    current_tree: Mutex<Option<AudioNode>>,
    loaded_audio_bytes: Mutex<Option<Vec<u8>>>,
    playback_state: Mutex<InternalPlaybackState>, // 🌟 State container loop
}

#[uniffi::export]
impl AudioEngine {
    #[uniffi::constructor]
    pub fn new() -> Self {
        AudioEngine {
            current_tree: Mutex::new(None),
            loaded_audio_bytes: Mutex::new(None),
            playback_state: Mutex::new(InternalPlaybackState {
                current_time_ms: 0,
                is_playing: false,
                repeat_mode: RepeatMode::Off,
                current_duration_ms: 0,
            }),
        }
    }

    pub fn get_audio_modules_path(&self) -> String {
        let proj_dirs = ProjectDirs::from("org", "flame", "xbible").expect("Path error");
        let path = proj_dirs.data_local_dir().join("modules").join("audio");
        fs::create_dir_all(&path).ok();
        path.to_string_lossy().into_owned()
    }

    pub fn get_audio_modules(&self) -> Vec<AudioModuleInfo> {
        let mut modules = Vec::new();
        let path_str = self.get_audio_modules_path();
        let path = Path::new(&path_str);

        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let file_path = entry.path();
                if file_path.is_file() && file_path.extension().map_or(false, |ext| ext.eq_ignore_ascii_case("xba")) {
                    if let Some(name_str) = file_path.file_name().and_then(|n| n.to_str()) {
                        let metadata = self.read_module_metadata_peek(&file_path);
                        let artwork = Artwork::new(file_path.to_string_lossy().into_owned());
                        modules.push(AudioModuleInfo {
                            file_name: name_str.to_string(),
                            absolute_path: file_path.to_string_lossy().into_owned(),
                            metadata,
                            artwork: Arc::new(artwork),
                        });
                    }
                }
            }
        }
        modules
    }

    pub fn peek_module_metadata(&self, file_path: String) -> Result<ModuleMetadata, AudioEngineError> {
        let path = Path::new(&file_path);
        if !path.exists() {
            return Err(AudioEngineError::ModuleNotFound { path: file_path });
        }
        self.read_module_metadata_peek(path)
            .ok_or_else(|| AudioEngineError::SerializationFailure {
                message: "Could not read metadata.json".to_string(),
            })
    }

    pub fn load_audio_module(&self, file_path: String) -> Result<Vec<u8>, AudioEngineError> {
        let path = Path::new(&file_path);
        if !path.exists() {
            return Err(AudioEngineError::ModuleNotFound { path: file_path });
        }

        let file = File::open(path)?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| AudioEngineError::IoFailure {
            message: format!("Invalid ZIP archive structure: {}", e),
        })?;

        let metadata: Option<ModuleMetadata> = self.read_module_metadata_peek(path);
        let duration = metadata.map(|m| m.duration_ms).unwrap_or(0);

        let root_tree: AudioNode = if let Ok(mut timestamps_file) = archive.by_name("timestamps.json") {
            let mut json_contents = String::new();
            timestamps_file.read_to_string(&mut json_contents)?;
            serde_json::from_str(&json_contents)?
        } else {
            return Err(AudioEngineError::IoFailure {
                message: "Missing required timestamps.json file".to_string(),
            });
        };

        let encrypted_audio_bytes = if let Ok(mut audio_file) = archive.by_name("audio.mp3") {
            let mut bytes = Vec::new();
            audio_file.read_to_end(&mut bytes)?;
            bytes
        } else {
            return Err(AudioEngineError::IoFailure {
                message: "Missing required audio.mp3 file".to_string(),
            });
        };

        let cipher = ChaCha20Poly1305::new(SECRET_KEY.into());
        let static_nonce_bytes = b"xbible_media";
        let nonce = Nonce::from_slice(static_nonce_bytes);

        let decrypted_audio_bytes = cipher
            .decrypt(nonce, encrypted_audio_bytes.as_slice())
            .map_err(|e| AudioEngineError::DecryptionFailure { message: e.to_string() })?;

        let mut tree_lock = self.current_tree.lock().unwrap();
        *tree_lock = Some(root_tree);

        let mut audio_lock = self.loaded_audio_bytes.lock().unwrap();
        *audio_lock = Some(decrypted_audio_bytes.clone());

        // Sync state context
        if let Ok(mut state) = self.playback_state.lock() {
            state.current_time_ms = 0;
            state.is_playing = false;
            state.current_duration_ms = duration;
        }

        Ok(decrypted_audio_bytes)
    }

    // =========================================================================
    // RUST CORE AUDIO TRANSPORT METHOD INTERFACES
    // =========================================================================

    /// Stop playback completely, resetting timestamps to 0 and dropping loaded audio references
    pub fn stop(&self) {
        if let Ok(mut state) = self.playback_state.lock() {
            state.is_playing = false;
            state.current_time_ms = 0;
        }
        // Wipe active hardware channels here if required
    }

    /// Skips forward cleanly by 30 seconds (30,000 milliseconds) capped to total duration
    pub fn skip_forward(&self) {
        if let Ok(mut state) = self.playback_state.lock() {
            let limit = state.current_duration_ms;
            state.current_time_ms = (state.current_time_ms + 30_000).min(limit);
        }
    }

    /// Backtracks cleanly by 15 seconds (15,000 milliseconds) bounded at 0
    pub fn skip_backward(&self) {
        if let Ok(mut state) = self.playback_state.lock() {
            state.current_time_ms = (state.current_time_ms - 15_000).max(0);
        }
    }

    /// Assign alternate repeat constraints directly onto the system core
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

    /// Unified state lookup pulling directly from current values managed inside Rust
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
        let tree_lock = self.current_tree.lock().unwrap();
        tree_lock.clone()
    }

    pub fn find_active_node_id(&self, time_ms: i64) -> Option<String> {
        let Some(ref root) = self.get_navigation_tree() else { return None; };
        if let Some(active_leaf) = find_active_verse_leaf(root, time_ms) {
            return Some(active_leaf.id.clone());
        }
        None
    }
}

// --- STANDARD INTERNAL HELPERS ---

impl AudioEngine {
    fn read_module_metadata_peek(&self, file_path: &Path) -> Option<ModuleMetadata> {
        let file = File::open(file_path).ok()?;
        let mut archive = zip::ZipArchive::new(file).ok()?;
        let mut meta_file = archive.by_name("metadata.json").ok()?;
        let mut contents = String::new();
        meta_file.read_to_string(&mut contents).ok()?;
        serde_json::from_str(&contents).ok()
    }
}

pub fn find_node_by_id<'a>(node: &'a AudioNode, id: &str) -> Option<&'a AudioNode> {
    if node.id == id { return Some(node); }
    for child in &node.children {
        if let Some(found) = find_node_by_id(child, id) { return Some(found); }
    }
    None
}

pub fn find_active_verse_leaf<'a>(node: &'a AudioNode, time_ms: i64) -> Option<&'a AudioNode> {
    if let (Some(start), Some(end)) = (node.start_ms, node.end_ms) {
        if time_ms >= start && time_ms <= end {
            if node.children.is_empty() { return Some(node); }
            for child in &node.children {
                if let Some(found) = find_active_verse_leaf(child, time_ms) { return Some(found); }
            }
            return Some(node);
        }
    } else {
        for child in &node.children {
            if let Some(found) = find_active_verse_leaf(child, time_ms) { return Some(found); }
        }
    }
    None
}