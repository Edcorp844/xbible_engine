use crate::engines::xbible_engine::engine::XBibleEngine;
use crate::engines::module_engine::module_engine::ModuleEngine;

/// Download progress details for module installation
#[derive(Debug, Clone, uniffi::Record)]
pub struct DownloadProgress {
    pub progress: f64,          // 0.0 → 1.0
    pub downloaded_bytes: i64,  // estimated
    pub total_bytes: i64,       // estimated
    pub current_module: String, // e.g. "nt.bzs" or full module name
    pub status: String,         // "downloading", "extracting", "complete", "error", "waiting"
    pub message: String,        // raw message from SWORD, e.g. "Downloading nt.bzs"
}

#[uniffi::export]
impl XBibleEngine {
    /// Get detailed download progress for module installation
    pub fn get_download_progress_details(&self) -> DownloadProgress {
        let progress_value = self.module_engine.get_download_progress();
        let raw_message = ModuleEngine::get_message();

        // Heuristic status from the message + progress
        let status = if progress_value >= 1.0 {
            "complete".to_string()
        } else if raw_message.to_lowercase().contains("error")
            || raw_message.to_lowercase().contains("failed")
        {
            "error".to_string()
        } else if raw_message.to_lowercase().contains("extract")
            || raw_message.to_lowercase().contains("install")
        {
            "extracting".to_string()
        } else if progress_value > 0.0 || !raw_message.is_empty() {
            "downloading".to_string()
        } else {
            "waiting".to_string()
        };

        // Try to pull a nice module name out of the message
        // Common patterns from SWORD: "Downloading nt.bzs", "Installing KJV", etc.
        let current_module = extract_module_name(&raw_message).unwrap_or_else(|| {
            if raw_message.is_empty() {
                "—".to_string()
            } else {
                raw_message.clone()
            }
        });

        // Still an estimate – SWORD only gives us a fraction via total/completed
        let total_bytes = 50_000_000i64; // ~50 MB default
        let downloaded_bytes = (progress_value * total_bytes as f64) as i64;

        DownloadProgress {
            progress: progress_value,
            downloaded_bytes,
            total_bytes,
            current_module,
            status,
            message: raw_message.into(),
        }
    }
}

/// Very simple helper to pull a filename / module name from typical SWORD messages
fn extract_module_name(msg: &str) -> Option<String> {
    // "Downloading nt.bzs"
    // "Downloading CrossWire/nt.bzs"
    // "Installing KJV..."
    let lower = msg.to_lowercase();
    if let Some(pos) = lower.find("downloading ") {
        let rest = msg[pos + "downloading ".len()..].trim();
        let name = rest
            .split_whitespace()
            .next()?
            .trim_matches(|c| c == '"' || c == '\'');
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    if let Some(pos) = lower.find("installing ") {
        let rest = msg[pos + "installing ".len()..].trim();
        let name = rest
            .split_whitespace()
            .next()?
            .trim_matches(|c| c == '"' || c == '\'');
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    None
}
