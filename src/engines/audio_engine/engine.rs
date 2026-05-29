use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;

use directories::ProjectDirs;

// --- ERROR HANDLING FOR UNIFFI COMPATIBILITY ---

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum AudioEngineError {
    #[error("Container package item not found: {path}")]
    ModuleNotFound { path: String },
    
    #[error("File system I/O error occurred: {message}")]
    IoFailure { message: String },
    
    #[error("Failed to parse structural JSON content format: {message}")]
    SerializationFailure { message: String },
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

// --- DATA STRUCTURE RECORDS ---

#[derive(Debug, Clone, uniffi::Record, serde::Deserialize)]
pub struct AudioTextAnchor {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
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
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct AudioModuleInfo {
    pub file_name: String,
    pub metadata: Option<ModuleMetadata>,
}

// --- 1. THE EXPLICIT RANGE TIMELINE TRACK ---

#[derive(Debug, Clone, uniffi::Record)]
pub struct RangeTrack {
    pub anchors: Vec<AudioTextAnchor>,
}

impl RangeTrack {
    /// Resolves the correct index matching the current time window.
    /// Returns -1 if the time falls into a gap where no text is mapped.
    pub fn current_anchor_index(&self, current_time_ms: i64) -> i32 {
        for (idx, anchor) in self.anchors.iter().enumerate() {
            if current_time_ms >= anchor.start_ms && current_time_ms <= anchor.end_ms {
                return idx as i32;
            }
        }
        -1 // In an empty gap or between verses
    }
}

// --- 2. THE AUDIO STATE RECORD ---

#[derive(Debug, Clone, uniffi::Record)]
pub struct PlaybackState {
    pub current_time_ms: i64,
    pub active_anchor_index: i32,
    pub active_text: String,
    pub is_playing: bool,
}

// --- 3. THE AUDIO ENGINE COORDINATOR ---

#[derive(uniffi::Object)]
pub struct AudioEngine {
    current_track: Mutex<Option<RangeTrack>>,
    loaded_audio_bytes: Mutex<Option<Vec<u8>>>,
}

// --- 1. THE EXPLICIT EXPORTED INTERFACE FOR SWIFT ---
// Keep only your public, FFI-compatible types and methods here.

#[uniffi::export]
impl AudioEngine {
    #[uniffi::constructor]
    pub fn new() -> Self {
        AudioEngine {
            current_track: Mutex::new(None),
            loaded_audio_bytes: Mutex::new(None),
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
                        // Calling across to our standard, non-exported helper block
                        let metadata = self.read_module_metadata_peek(&file_path);
                        
                        modules.push(AudioModuleInfo {
                            file_name: name_str.to_string(),
                            metadata,
                        });
                    }
                }
            }
        }
        modules
    }

    pub fn get_current_playback_state(&self, current_time_ms: i64, is_playing: bool) -> Option<PlaybackState> {
        let track_lock = self.current_track.lock().unwrap();
        let track = track_lock.as_ref()?;
        let active_anchor_index = track.current_anchor_index(current_time_ms);

        let active_text = if active_anchor_index >= 0 && (active_anchor_index as usize) < track.anchors.len() {
            track.anchors[active_anchor_index as usize].text.clone()
        } else {
            String::new()
        };

        Some(PlaybackState {
            current_time_ms,
            active_anchor_index,
            active_text,
            is_playing,
        })
    }

    pub fn load_audio_module(&self, file_path: String) -> Result<Vec<u8>, AudioEngineError> {
        let path = Path::new(&file_path);
        if !path.exists() {
            return Err(AudioEngineError::ModuleNotFound { path: file_path });
        }

        let file = File::open(path)?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| AudioEngineError::IoFailure { message: format!("Invalid ZIP: {}", e) })?;

        let range_track = if let Ok(mut timestamps_file) = archive.by_name("timestamps.json") {
            let mut json_contents = String::new();
            timestamps_file.read_to_string(&mut json_contents)?;
            let anchors: Vec<AudioTextAnchor> = serde_json::from_str(&json_contents)?;
            RangeTrack { anchors }
        } else {
            return Err(AudioEngineError::IoFailure { message: "Missing timestamps.json".to_string() });
        };

        let audio_bytes = if let Ok(mut audio_file) = archive.by_name("audio.mp3") {
            let mut bytes = Vec::new();
            audio_file.read_to_end(&mut bytes)?;
            bytes
        } else {
            return Err(AudioEngineError::IoFailure { message: "Missing audio.mp3".to_string() });
        };

        let mut track_lock = self.current_track.lock().unwrap();
        *track_lock = Some(range_track);

        let mut audio_lock = self.loaded_audio_bytes.lock().unwrap();
        *audio_lock = Some(audio_bytes.clone());

        Ok(audio_bytes)
    }

    pub fn update_playback_sync(&self, current_time_ms: i64, is_playing: bool) -> Option<PlaybackState> {
        let track_lock = self.current_track.lock().unwrap();
        let track = track_lock.as_ref()?;
        let active_anchor_index = track.current_anchor_index(current_time_ms);

        let active_text = if active_anchor_index >= 0 && (active_anchor_index as usize) < track.anchors.len() {
            track.anchors[active_anchor_index as usize].text.clone()
        } else {
            String::new()
        };

        Some(PlaybackState {
            current_time_ms,
            active_anchor_index,
            active_text,
            is_playing,
        })
    }
}

// --- 2. THE STANDARD RUST INTERNAL IMPLEMENTATION BLOCK ---
// This is invisible to UniFFI. You can use standard Rust types like Path without issues.
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