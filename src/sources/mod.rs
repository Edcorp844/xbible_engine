//! Generic, source-agnostic ingestion models, source contracts, and registry (`src/sources/mod.rs`).
//!
//! Each provider (`helloao`, `fcbh`, `api_bible`, `open_bible`) owns its own
//! HTTP client, authentication/endpoints, and raw JSON deserialization structs
//! internally, then normalizes payloads into the `Generic*` types defined here
//! before writing them into the database in a form consumed by the Reader.
//!
//! UniFFI & concurrency notes:
//! - Exposed data types derive `uniffi::Record` or `uniffi::Enum` using owned
//!   types (`String`, `Vec`, `Option`, `bool`, `i32`, `i64`).
//! - Nested chapter collections use `Vec<GenericBookChapters>` instead of maps
//!   to keep cross-language FFI bindings straightforward.
//! - `SourceRegistry` uses `RwLock` so sources can be registered or queried
//!   safely from background/store threads without requiring `&mut self` everywhere.

pub mod helloao;

use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::db::install::{persist_installed_bible, uninstall_translation};
use crate::error::XBibleError;

// ═════════════════════════════════════════════════════════════════════════════
// 1. Direction, Canon & Inline Span Models
// ═════════════════════════════════════════════════════════════════════════════

/// Reading flow direction for text in a source resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Enum)]
pub enum TextDirection {
    /// Left-to-right script flow (e.g., Latin or Greek).
    #[default]
    LeftToRight,
    /// Right-to-right script flow (e.g., Arabic or Hebrew).
    RightToLeft,
}

/// Canonical book grouping used for navigation, filtering, and canon handling.
#[derive(Debug, Clone, PartialEq, Eq, Default, uniffi::Enum)]
pub enum CanonicalSection {
    /// Standard Old Testament books.
    OldTestament,
    /// Standard New Testament books.
    NewTestament,
    /// Deuterocanonical / apocryphal books.
    Deuterocanon,
    /// Books specific to Orthodox canons.
    OrthodoxExtra,
    /// Books specific to Ethiopian canons.
    EthiopianExtra,
    /// Fallback when a source does not provide a canonical section.
    #[default]
    Unknown,
    /// Source- or edition-specific section label outside standard buckets.
    Custom(String),
}

/// Semantic role of an inline span inside a chapter item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Enum, Serialize)]
pub enum SpanKind {
    /// Plain prose or narrative text segment.
    #[default]
    Text,
    /// Individual word token (e.g., when lexical or word-level alignment exists).
    Word,
    /// Inline marker linking to a chapter footnote.
    FootnoteRef,
    /// Inline marker linking to a cross-reference.
    CrossRef,
    /// Direct quotation or spoken text span.
    Quote,
    /// Any other normalized inline span type.
    Other,
}

/// Visual and typographic flags applied to an inline span.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record, Serialize)]
pub struct SpanFormatting {
    /// True when rendered in red-letter / words-of-Jesus styling.
    pub is_words_of_jesus: bool,
    /// True when italicized (e.g., added/supplied words or emphasis).
    pub is_italic: bool,
    /// True when rendered in bold weight.
    pub is_bold: bool,
    /// True when rendered in small caps.
    pub is_small_caps: bool,
    /// True when underlined.
    pub is_underline: bool,
    /// True when styled as poetic/verse line text.
    pub is_poetry: bool,
}

/// Optional original-language and lexical metadata attached to a span.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record, Serialize)]
pub struct LexicalInfo {
    /// Strong's concordance number (e.g., `"H430"` or `"G26"`).
    pub strongs: Option<String>,
    /// Base/dictionary lemma form.
    pub lemma: Option<String>,
    /// Phonetic or Latin transliteration.
    pub transliteration: Option<String>,
    /// Part-of-speech tag.
    pub part_of_speech: Option<String>,
    /// Morphological parsing code or label.
    pub morphology: Option<String>,
    /// Concise gloss or meaning.
    pub gloss: Option<String>,
}

/// Inline text run inside a `GenericChapterItem`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Serialize)]
pub struct GenericTextSpan {
    /// Ordering index within the parent chapter item.
    pub position: i32,
    /// Semantic role of this span.
    pub kind: SpanKind,
    /// Text content of this span.
    pub text: String,
    /// Typographic styling flags for rendering.
    pub formatting: SpanFormatting,
    /// Optional lexical/Strong's/morphology data when available.
    pub lexical: Option<LexicalInfo>,
    /// Identifiers of footnotes referenced by this span.
    pub footnote_ids: Vec<String>,
    /// Identifiers of cross-references linked from this span.
    pub cross_ref_ids: Vec<String>,
    /// Optional ISO/BCP-47 language code when different from the chapter/book.
    pub language_code: Option<String>,
}

// ═════════════════════════════════════════════════════════════════════════════
// 2. Audio & Timing Models
// ═════════════════════════════════════════════════════════════════════════════

/// Descriptor for an audio stream, fileset, or track (supports FCBH/Bible Brain
/// filesets as well as chapter/book audio URLs or stream IDs).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Serialize, Deserialize)]
pub struct GenericAudioSource {
    /// Provider-specific fileset ID, stream ID, or track key.
    pub source_ref_id: String,
    /// Optional direct URL or path template when provided by the source.
    pub url: Option<String>,
    /// Optional audio format or container hint (e.g., `"mp3"`, `"opus"`, `"aac"`).
    pub format: Option<String>,
    /// Whether the recording is dramatized (`Some(true)`), non-dramatized
    /// (`Some(false)`), or unspecified (`None`).
    pub is_dramatized: Option<bool>,
}

/// Per-verse audio timestamp range in milliseconds.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericAudioTiming {
    /// Target verse number within the chapter.
    pub verse: i32,
    /// Start offset in milliseconds from the beginning of the audio.
    pub start_ms: Option<i64>,
    /// End offset in milliseconds from the beginning of the audio.
    pub end_ms: Option<i64>,
}

// ═════════════════════════════════════════════════════════════════════════════
// 3. Catalog, Book, Chapter & Install Bundle Models
// ═════════════════════════════════════════════════════════════════════════════

/// Catalog metadata for a Bible resource discovered from a source.
///
/// Counts use `Option<i32>` so sources that omit totals in catalog listings
/// (such as some REST or fileset APIs) do not need placeholder numbers.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericCatalogBible {
    /// Resource identifier within the source.
    pub id: String,
    /// Source provider key that owns this resource (e.g., `"helloao"`, `"fcbh"`).
    pub source_id: String,
    /// Primary display name.
    pub name: String,
    /// English display name if available.
    pub english_name: Option<String>,
    /// Short abbreviation or sigla if available.
    pub short_name: Option<String>,
    /// Primary language code.
    pub language_code: String,
    /// Native language name if reported.
    pub language_name: Option<String>,
    /// English language name if reported.
    pub language_english_name: Option<String>,
    /// Script reading direction.
    pub text_direction: TextDirection,
    /// Number of books in the resource, if known.
    pub book_count: Option<i32>,
    /// Number of chapters in the resource, if known.
    pub chapter_count: Option<i32>,
    /// Number of verses in the resource, if known.
    pub verse_count: Option<i32>,
    /// True if text content is available from this resource.
    pub has_text: bool,
    /// True if audio content or filesets are available from this resource.
    pub has_audio: bool,
    /// Whether lexical/Strong's annotations are present, if known.
    pub has_strongs: Option<bool>,
    /// Licensing or terms URL, if provided.
    pub license_url: Option<String>,
    /// Human-readable license or attribution notes, if provided.
    pub license_notes: Option<String>,
    /// Associated audio sources/filesets for this Bible.
    pub audio_sources: Vec<GenericAudioSource>,
}

/// Normalized book metadata within a Bible resource.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericBook {
    /// Owning Bible/translation identifier.
    pub translation_id: String,
    /// Source provider key that owns this book.
    pub source_id: String,
    /// Book identifier (e.g., `"GEN"`, `"JHN"`, or source-specific book key).
    pub id: String,
    /// Display name of the book.
    pub name: String,
    /// Common or alternate book name, if available.
    pub common_name: Option<String>,
    /// Formal or full book title, if available.
    pub title: Option<String>,
    /// Canonical or source sort order, if available.
    pub book_order: Option<i32>,
    /// Canonical section classification.
    pub section: CanonicalSection,
    /// Total number of chapters in this book.
    pub number_of_chapters: i32,
    /// Book-level audio sources/filesets if applicable.
    pub audio_sources: Vec<GenericAudioSource>,
}

/// Ordered structural block within a chapter (verse, heading, line break, etc.).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record, Serialize)]
pub struct GenericChapterItem {
    /// Ordering position within the chapter.
    pub position: i32,
    /// Normalized item type (e.g., `"verse"`, `"heading"`, `"line_break"`).
    pub item_type: String,
    /// Verse number when this item represents or belongs to a verse.
    pub verse_number: Option<i32>,
    /// Plain text summary/content for the item when applicable.
    pub text: Option<String>,
    /// True if formatted as poetry.
    pub is_poem: bool,
    /// True if containing words of Jesus.
    pub is_words_of_jesus: bool,
    /// Footnote IDs referenced by this item.
    pub footnote_ids: Vec<String>,
    /// Structured inline spans that make up this item.
    pub spans: Vec<GenericTextSpan>,
}

/// Normalized verse entry for lookup and full-text search indexing.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericVerse {
    /// Book identifier containing the verse.
    pub book_id: String,
    /// One-based chapter number.
    pub chapter: i32,
    /// One-based verse number.
    pub verse: i32,
    /// Normalized plain text of the verse.
    pub text: String,
}

/// Footnote entry belonging to a chapter.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericFootnote {
    /// Footnote identifier referenced by chapter items or spans.
    pub note_id: String,
    /// Optional marker/caller symbol shown at the anchor.
    pub caller: Option<String>,
    /// Footnote text body.
    pub text: Option<String>,
    /// Optional scripture or external reference cited in the note.
    pub reference: Option<String>,
}

/// Normalized chapter payload ready to be written into the database.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericChapter {
    /// Owning Bible/translation identifier.
    pub translation_id: String,
    /// Source provider key that produced this chapter.
    pub source_id: String,
    /// Owning book identifier.
    pub book_id: String,
    /// One-based chapter number.
    pub chapter_number: i32,
    /// Ordered structural blocks in the chapter.
    pub items: Vec<GenericChapterItem>,
    /// Normalized verse rows for lookup and search.
    pub verses: Vec<GenericVerse>,
    /// Footnotes belonging to this chapter.
    pub footnotes: Vec<GenericFootnote>,
    /// Chapter- or verse-level audio sources/filesets.
    pub audio_sources: Vec<GenericAudioSource>,
    /// Per-verse audio timing offsets for this chapter.
    pub audio_timings: Vec<GenericAudioTiming>,
}

/// Groups all normalized chapters for a single book.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericBookChapters {
    /// Book identifier (`GenericBook::id`).
    pub book_id: String,
    /// Chapters belonging to this book.
    pub chapters: Vec<GenericChapter>,
}

/// Complete normalized Bible bundle returned by a `BibleSource` for installation.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericInstalledBible {
    /// Catalog metadata and capabilities for the Bible.
    pub catalog: GenericCatalogBible,
    /// Books in the Bible.
    pub books: Vec<GenericBook>,
    /// Chapters grouped by book ID.
    pub chapters_by_book: Vec<GenericBookChapters>,
}

// ═════════════════════════════════════════════════════════════════════════════
// 4. Source Contract, Installer & Registry
// ═════════════════════════════════════════════════════════════════════════════

/// Progress and cancellation interface passed to a `BibleSource` during
/// multi-step download or installation work.
pub trait InstallProgressSink: Send + Sync {
    /// Reports current progress for a stage (`current` out of `total`).
    fn on_progress(&self, stage: String, current: u32, total: u32, message: Option<String>);

    /// Returns true if the caller/store requested cancellation.
    fn is_cancelled(&self) -> bool;
}

/// Provider abstraction isolating upstream API differences behind normalized
/// `Generic*` types.
///
/// Store/engine code calls `BibleSource` methods without knowing endpoint paths,
/// auth headers, pagination rules, or raw JSON schemas.
pub trait BibleSource: Send + Sync {
    /// Unique identifier for this source (e.g., `"helloao"`, `"fcbh"`, `"api_bible"`, `"open_bible"`).
    fn source_id(&self) -> &str;

    /// Human-readable name for UI or logging.
    fn display_name(&self) -> &str;

    /// Fetches available Bible catalog entries from this source.
    fn list_catalog(&self) -> Result<Vec<GenericCatalogBible>, String>;

    /// Fetches book metadata for a specific Bible ID within this source.
    fn list_books(&self, bible_id: &str) -> Result<Vec<GenericBook>, String>;

    /// Fetches and normalizes a single chapter on demand.
    fn fetch_chapter(
        &self,
        bible_id: &str,
        book_id: &str,
        chapter_number: i32,
    ) -> Result<GenericChapter, String>;

    /// Downloads, normalizes, and returns a full Bible bundle ready for DB persistence.
    fn fetch_for_install(
        &self,
        bible_id: &str,
        progress: &dyn InstallProgressSink,
    ) -> Result<GenericInstalledBible, String>;
}

/// Source-level installation contract so each provider installs and uninstalls
/// its own Bible items using normalized persistence helpers.
pub trait SourceInstaller: BibleSource {
    /// Fetches and persists a complete Bible bundle into SQLite.
    fn install_bible(
        &self,
        conn: &Connection,
        bible_id: &str,
        progress: &dyn InstallProgressSink,
    ) -> Result<GenericInstalledBible, XBibleError> {
        let bundle: GenericInstalledBible = self
            .fetch_for_install(bible_id, progress)
            .map_err(|msg: String| XBibleError::Generic { error_message: msg })?;
        persist_installed_bible(conn, &bundle)?;
        Ok(bundle)
    }

    /// Removes installed content for `bible_id` and resets its `installed` flag.
    fn uninstall_bible(&self, conn: &Connection, bible_id: &str) -> Result<(), XBibleError> {
        uninstall_translation(conn, bible_id)
    }
}

impl<T: BibleSource + ?Sized> SourceInstaller for T {}

/// Thread-safe registry of configured `BibleSource` implementations.
#[derive(Default)]
pub struct SourceRegistry {
    sources: RwLock<HashMap<String, Arc<dyn BibleSource>>>,
}

impl SourceRegistry {
    /// Creates an empty source registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers or replaces a source keyed by `source.source_id()`.
    pub fn register(&self, source: Arc<dyn BibleSource>) {
        let key: String = source.source_id().to_string();
        if let Ok(mut guard) = self.sources.write() {
            let guard: &mut RwLockWriteGuard<'_, HashMap<String, Arc<dyn BibleSource>>> =
                &mut guard;
            guard.insert(key, source);
        }
    }

    /// Looks up a registered source by `source_id`.
    pub fn get(&self, source_id: &str) -> Option<Arc<dyn BibleSource>> {
        self.sources.read().ok().and_then(
            |guard: RwLockReadGuard<'_, HashMap<String, Arc<dyn BibleSource>>>| {
                guard.get(source_id).cloned()
            },
        )
    }

    /// Returns a snapshot of all registered sources.
    pub fn all(&self) -> Vec<Arc<dyn BibleSource>> {
        self.sources
            .read()
            .ok()
            .map(
                |guard: RwLockReadGuard<'_, HashMap<String, Arc<dyn BibleSource>>>| {
                    guard
                        .values()
                        .cloned()
                        .collect::<Vec<Arc<dyn BibleSource>>>()
                },
            )
            .unwrap_or_default()
    }
}
