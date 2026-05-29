use crate::engines::xbible_engine::engine::XBibleEngine;

/// Download progress details for module installation
#[derive(Debug, Clone, uniffi::Record)]
pub struct DownloadProgress {
    pub progress: f64,          // 0.0 to 1.0
    pub downloaded_bytes: i64,  // Bytes downloaded so far
    pub total_bytes: i64,       // Total bytes to download
    pub current_module: String, // Name of module being downloaded
    pub status: String,         // "downloading", "extracting", "complete", "error"
}

#[uniffi::export]
impl XBibleEngine {
    /// Get detailed download progress for module installation
    pub fn get_download_progress_details(&self) -> DownloadProgress {
        let progress_value = self.module_engine.get_download_progress();
        // Calculate bytes based on progress (this is an estimate)
        let total_bytes = 100_000_000i64; // Default estimate: 100MB
        let downloaded_bytes = (progress_value * total_bytes as f64) as i64;

        let status = if progress_value >= 1.0 {
            "complete".to_string()
        } else if progress_value > 0.0 {
            "downloading".to_string()
        } else {
            "waiting".to_string()
        };

        DownloadProgress {
            progress: progress_value,
            downloaded_bytes,
            total_bytes,
            current_module: String::from("xbible_engine"),
            status,
        }
    }
}
