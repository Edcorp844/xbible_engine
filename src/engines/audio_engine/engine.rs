use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

// --- 1. THE LYRICS STRUCTURES ---

#[derive(Debug, Clone, uniffi::Record)]
pub struct LyricLine {
    pub timestamp_ms: i64,
    pub text: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct LyricTrack {
    pub lines: Vec<LyricLine>,
}

impl LyricTrack {
    pub fn parse_lrc(raw_content: &str) -> Self {
        let mut sorted_timeline = BTreeMap::new();
        for line in raw_content.lines() {
            let trimmed = line.trim();
            if let Some(end_bracket_idx) = trimmed.find(']') {
                if trimmed.starts_with('[') {
                    let timestamp_part = &trimmed[1..end_bracket_idx];
                    let text_part = &trimmed[end_bracket_idx + 1..];

                    if let Some(parts) = timestamp_part.split_once(':') {
                        let minutes: i64 = parts.0.parse().unwrap_or(0);
                        let seconds: f64 = parts.1.parse().unwrap_or(0.0);
                        let total_ms = (minutes * 60 * 1000) + (seconds * 1000.0) as i64;
                        sorted_timeline.insert(total_ms, text_part.trim().to_string());
                    }
                }
            }
        }
        let lines = sorted_timeline
            .into_iter()
            .map(|(timestamp_ms, text)| LyricLine { timestamp_ms, text })
            .collect();

        LyricTrack { lines }
    }

    pub fn current_line_index(&self, current_time_ms: i64) -> i32 {
        if self.lines.is_empty() { return -1; }
        match self.lines.binary_search_by_key(&current_time_ms, |line| line.timestamp_ms) {
            Ok(exact_index) => exact_index as i32,
            Err(insertion_index) => {
                if insertion_index > 0 { (insertion_index - 1) as i32 } else { 0 }
            }
        }
    }
}

// --- 2. THE CHANNELS BRIDGE (THE TRAIT INTERFACE) ---

#[uniffi::export(with_foreign)]
pub trait NativeAudioPlayer: Send + Sync {
    fn get_current_time_ms(&self) -> i64;
    fn is_playing(&self) -> bool;
}

// --- 3. THE AUDIO ENGINE COORDINATOR ---

#[derive(Debug, Clone, uniffi::Record)]
pub struct PlaybackState {
    pub current_time_ms: i64,
    pub active_lyric_index: i32,
    pub active_text: String,
    pub is_playing: bool,
}

#[derive(uniffi::Object)]
pub struct AudioEngine {
    // FIX: Wrap the player in a Mutex to allow thread-safe interior mutability
    native_player: Mutex<Option<Arc<dyn NativeAudioPlayer>>>,
    current_track: Mutex<Option<LyricTrack>>,
}

#[uniffi::export]
impl AudioEngine {
    #[uniffi::constructor]
    pub fn new() -> Self {
        AudioEngine {
            // FIX: Initialize with a Mutex
            native_player: Mutex::new(None),
            current_track: Mutex::new(None),
        }
    }

    // FIX: Changed from `&mut self` to `&self` so UniFFI's Arc wrapper works seamlessly
    pub fn register_player(&self, player: Arc<dyn NativeAudioPlayer>) {
        let mut player_lock = self.native_player.lock().unwrap();
        *player_lock = Some(player);
    }

    pub fn load_lyrics(&self, raw_lrc: &str) {
        let track = LyricTrack::parse_lrc(raw_lrc);
        let mut current = self.current_track.lock().unwrap();
        *current = Some(track);
    }

    pub fn get_playback_state(&self) -> Option<PlaybackState> {
        // FIX: Acquire lock before reading the shared native player reference
        let player_lock = self.native_player.lock().unwrap();
        let player = player_lock.as_ref()?;
        
        let track_lock = self.current_track.lock().unwrap();
        let track = track_lock.as_ref()?;

        let current_time_ms = player.get_current_time_ms();
        let is_playing = player.is_playing();
        let active_lyric_index = track.current_line_index(current_time_ms);
        
        let active_text = if active_lyric_index >= 0 && (active_lyric_index as usize) < track.lines.len() {
            track.lines[active_lyric_index as usize].text.clone()
        } else {
            String::new()
        };

        Some(PlaybackState {
            current_time_ms,
            active_lyric_index,
            active_text,
            is_playing,
        })
    }
}
