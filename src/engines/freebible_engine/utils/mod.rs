#[derive(Debug, Clone, uniffi::Record)]
pub struct InstallProgress {
    /// 0.0 → 1.0, for the top-level progress bar
    pub fraction: f64,

    /// Chapters completed so far
    pub chapters_done: i32,

    /// Total chapters in this translation
    pub chapters_total: i32,

    /// Current book being downloaded (e.g. "Genesis")
    pub current_book_name: String,

    /// Current book id (e.g. "GEN")
    pub current_book_id: String,

    /// Current chapter number being downloaded
    pub current_chapter: i32,

    /// Verses downloaded in the current chapter
    pub current_chapter_verses: i32,

    /// Verses downloaded so far, across the whole translation
    pub verses_done: i32,

    /// Human-readable summary the UI can display as-is
    pub message: String,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum FreeBibleTaskState {
    Queued,
    Running,
    Completed,
    Failed { error: String },
    Cancelled,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FreeBibleTaskStatus {
    pub task_id: String,
    pub state: FreeBibleTaskState,
    pub progress: InstallProgress,
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FreeBibleError {
    #[error("{error_message}")]
    Generic { error_message: String },
}

impl From<String> for FreeBibleError {
    fn from(s: String) -> Self {
        FreeBibleError::Generic { error_message: s }
    }
}

impl From<rusqlite::Error> for FreeBibleError {
    fn from(e: rusqlite::Error) -> Self {
        FreeBibleError::Generic { error_message: e.to_string() }
    }
}