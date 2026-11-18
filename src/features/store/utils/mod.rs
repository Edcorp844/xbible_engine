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



#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// Bible translation (BSB, KJV, WEB, etc.)
    /// URL: /api/{id}/... or /api/{id}/complete.json
    Translation,

    /// Commentary (Adam Clarke, Tyndale, etc.)
    /// URL: /api/c/{id}/...
    /// No complete.json — must use per-chapter
    Commentary,

    /// Dataset (stepbible-lexicon, bsb-interlinear, open-cross-ref, theographic, etc.)
    /// URL: /api/d/{id}/...
    /// Has its own sub-shapes: lexicon entries vs chapter-aligned words/entities
    Dataset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatasetKind {
    /// Keyed by Strong's number. Install by iterating lexicon entries.
    Lexicon,
    /// Chapter-aligned words. Install alongside the translation it belongs to.
    Interlinear,
    /// Chapter-aligned entities (people/places/events).
    Entities,
    /// Unknown — skip install.
    Unsupported,
}