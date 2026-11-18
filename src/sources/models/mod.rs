//! Generic, source-agnostic translation and scholarly models for database
//! storage and cross-language FFI transfer via UniFFI.
//!
//! Sources (such as HelloAO, FCBH/Bible Brain, or future providers) normalize
//! upstream payloads into these types so the store, reader, search engine,
//! and UI remain insulated from wire-format differences.
//!
//! UniFFI compatibility notes:
//! - Every exposed struct/enum derives `uniffi::Record` or `uniffi::Enum`
//!   using owned types (`String`, `Vec`, `Option`, `bool`, `i32`, `i64`).
//! - Nested chapter collections use `Vec<GenericBookChapters>` rather than
//!   `HashMap<String, Vec<GenericChapter>>` so FFI bindings remain uniform
//!   and predictable across targets.

use serde::{Deserialize, Serialize};

// ── 1. Direction, Canon & Inline Span Models ─────────────────────────────────

/// Reading flow direction for text in a source resource.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, uniffi::Enum, Serialize, Deserialize,
)]
pub enum TextDirection {
    /// Left-to-right script flow (e.g., Latin or Greek).
    #[default]
    LeftToRight,
    /// Right-to-right script flow (e.g., Arabic or Hebrew).
    RightToLeft,
}

/// Canonical book grouping used for navigation, filtering, and canon handling.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, uniffi::Enum, Serialize, Deserialize)]
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
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, uniffi::Enum, Serialize, Deserialize,
)]
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
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, uniffi::Record, Serialize, Deserialize,
)]
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

/// Original-language and lexical annotations attached to a span or search result.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, uniffi::Record, Serialize, Deserialize)]
pub struct LexicalInfo {
    /// Strong's concordance identifier (e.g., `"H430"` or `"G26"`).
    pub strongs: Option<String>,
    /// Base/dictionary lemma form.
    pub lemma: Option<String>,
    /// Phonetic or Latin transliteration.
    pub transliteration: Option<String>,
    /// Grammatical part-of-speech tag.
    pub part_of_speech: Option<String>,
    /// Morphological parsing code or label.
    pub morphology: Option<String>,
    /// Concise gloss or translation equivalent.
    pub gloss: Option<String>,
}

/// Normalized inline text segment inside a `GenericChapterItem`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, uniffi::Record, Serialize, Deserialize)]
pub struct GenericTextSpan {
    /// Zero- or one-based ordering index within the parent chapter item.
    pub position: i32,
    /// Semantic role of this span within the block.
    pub kind: SpanKind,
    /// Visible text content of the span.
    pub text: String,
    /// Typographic styling flags for rendering.
    pub formatting: SpanFormatting,
    /// Optional lexical/Strong's/morphology metadata when available.
    pub lexical: Option<LexicalInfo>,
    /// Identifiers of footnotes referenced by this span.
    pub footnote_ids: Vec<String>,
    /// Identifiers of cross-references linked from this span.
    pub cross_ref_ids: Vec<String>,
    /// Optional ISO/BCP-47 language code when different from the chapter/book.
    pub language_code: Option<String>,
    /// Optional source-specific payload retained for inspection or re-parsing.
    pub raw_json: Option<String>,
}

// ── 2. Audio & Timing Models ─────────────────────────────────────────────────

/// Descriptor for an audio stream, fileset, or track (supports FCBH/Bible Brain
/// filesets as well as chapter/book audio URLs or stream IDs).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, uniffi::Record, Serialize, Deserialize)]
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

/// Per-verse audio timestamp range in milliseconds within an audio-backed chapter.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, uniffi::Record, Serialize, Deserialize,
)]
pub struct GenericAudioTiming {
    /// Target verse number within the chapter.
    pub verse: i32,
    /// Start offset in milliseconds from the beginning of the audio.
    pub start_ms: Option<i64>,
    /// End offset in milliseconds from the beginning of the audio.
    pub end_ms: Option<i64>,
}

// ── 3. Catalog, Book, Chapter & Install Bundle Models ────────────────────────

/// Catalog metadata for a Bible resource discovered from a source.
///
/// Counts use `Option<i32>` so sources that omit totals in catalog listings
/// (such as some REST or fileset APIs) do not need placeholder numbers.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, uniffi::Record, Serialize, Deserialize)]
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

/// Catalog and capability record for a Bible translation stored in the DB.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, uniffi::Record, Serialize, Deserialize)]
pub struct GenericTranslation {
    /// Translation identifier (unique within the source or namespaced key).
    pub id: String,
    /// Identifier of the source provider supplying this translation (e.g., `"helloao"`).
    pub source_id: String,
    /// Native or primary display name of the translation.
    pub name: String,
    /// English display name of the translation.
    pub english_name: String,
    /// Short abbreviation or sigla (e.g., `"ESV"`, `"KJV"`, `"BSB"`).
    pub short_name: String,
    /// Primary language code of the translation.
    pub language_code: String,
    /// Native language name if reported by the source.
    pub language_name: Option<String>,
    /// English language name if reported by the source.
    pub language_english_name: Option<String>,
    /// Reading direction for the translation's text.
    pub text_direction: TextDirection,
    /// Total number of books in the translation.
    pub book_count: i32,
    /// Total number of chapters across all books.
    pub chapter_count: i32,
    /// Total number of verses across all chapters.
    pub verse_count: i32,
    /// Whether full text/chapter content is currently installed locally.
    pub installed: bool,
    /// Whether Strong's or original-language lexical tags are present.
    pub has_strongs: bool,
    /// Optional URL pointing to licensing or terms of use.
    pub license_url: Option<String>,
    /// Optional human-readable license or attribution notes.
    pub license_notes: Option<String>,
    /// Optional raw JSON metadata preserved from the source catalog.
    pub raw_json: Option<String>,
}

/// Normalized book-level structure and metadata within a Bible resource.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, uniffi::Record, Serialize, Deserialize)]
pub struct GenericBook {
    /// Owning Bible/translation identifier.
    pub translation_id: String,
    /// Source provider key that owns this book.
    pub source_id: String,
    /// Book identifier (e.g., `"GEN"`, `"JHN"`, `"REV"`, or source-specific key).
    pub id: String,
    /// Standard book name for display.
    pub name: String,
    /// Common or alternate book name, if available.
    pub common_name: Option<String>,
    /// Formal or full book title, if available.
    pub title: Option<String>,
    /// Canonical or source sort order, if available.
    pub book_order: Option<i32>,
    /// Canonical section classification used for canon filtering and grouping.
    pub section: CanonicalSection,
    /// Total number of chapters in this book.
    pub number_of_chapters: i32,
    /// Book-level audio sources/filesets if applicable.
    pub audio_sources: Vec<GenericAudioSource>,
    /// Optional JSON-encoded audio resource links for the book.
    pub audio_links_json: Option<String>,
    /// Optional raw JSON metadata preserved from the source.
    pub raw_json: Option<String>,
}

/// Ordered structural block within a chapter (verse, heading, line break, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, uniffi::Record, Serialize, Deserialize)]
pub struct GenericChapterItem {
    /// Ordering position within the chapter.
    pub position: i32,
    /// Normalized item category (e.g., `"verse"`, `"heading"`, `"line_break"`).
    pub item_type: String,
    /// Verse number when this item represents or belongs to a verse.
    pub verse_number: Option<i32>,
    /// Plain-text summary/content for the item when applicable.
    pub text: Option<String>,
    /// True if the block represents poetic lines.
    pub is_poem: bool,
    /// True if the block contains words of Jesus.
    pub is_words_of_jesus: bool,
    /// Footnote IDs referenced by this item.
    pub footnote_ids: Vec<String>,
    /// Structured inline spans making up this item.
    pub spans: Vec<GenericTextSpan>,
    /// Optional source-specific JSON retained for inspection or re-parsing.
    pub raw_json: Option<String>,
}

/// Normalized verse entry for direct lookup and full-text search indexing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, uniffi::Record, Serialize, Deserialize)]
pub struct GenericVerse {
    /// Book identifier containing this verse.
    pub book_id: String,
    /// One-based chapter number.
    pub chapter: i32,
    /// One-based verse number.
    pub verse: i32,
    /// Merged/normalized plain text of the verse.
    pub text: String,
}

/// Footnote entry belonging to a chapter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, uniffi::Record, Serialize, Deserialize)]
pub struct GenericFootnote {
    /// Footnote identifier referenced by chapter items or spans.
    pub note_id: String,
    /// Optional marker/caller symbol shown at the anchor (e.g., `"a"` or `"*"`).
    pub caller: Option<String>,
    /// Footnote text body.
    pub text: Option<String>,
    /// Optional scripture or external reference cited in the note.
    pub reference: Option<String>,
    /// Optional raw JSON payload from the source.
    pub raw_json: Option<String>,
}

/// Complete normalized chapter payload ready for DB persistence.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, uniffi::Record, Serialize, Deserialize)]
pub struct GenericChapter {
    /// Owning Bible/translation identifier.
    pub translation_id: String,
    /// Source provider key that produced this chapter.
    pub source_id: String,
    /// Owning book identifier.
    pub book_id: String,
    /// One-based chapter number.
    pub chapter_number: i32,
    /// Ordered structural blocks/items in the chapter.
    pub items: Vec<GenericChapterItem>,
    /// Normalized verse rows for lookup and search indexing.
    pub verses: Vec<GenericVerse>,
    /// Footnotes belonging to this chapter.
    pub footnotes: Vec<GenericFootnote>,
    /// Chapter- or verse-level audio sources/filesets.
    pub audio_sources: Vec<GenericAudioSource>,
    /// Per-verse audio timing offsets for this chapter.
    pub audio_timings: Vec<GenericAudioTiming>,
    /// Raw chapter JSON preserved as a safety net so future parsers can re-extract fields.
    pub raw_json: String,
}

/// Pairs a book identifier with all normalized chapters belonging to that book.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, uniffi::Record, Serialize, Deserialize)]
pub struct GenericBookChapters {
    /// Book identifier (`GenericBook::id`).
    pub book_id: String,
    /// Chapters for this book in chapter order.
    pub chapters: Vec<GenericChapter>,
}

/// Complete normalized bundle returned by a `BibleSource` for installation
/// into the general reader representation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, uniffi::Record, Serialize, Deserialize)]
pub struct GenericInstalledBible {
    /// Catalog metadata and capabilities for the resource.
    pub catalog: GenericCatalogBible,
    /// Books or sections in the resource.
    pub books: Vec<GenericBook>,
    /// Chapters or entries grouped by book ID.
    pub chapters_by_book: Vec<GenericBookChapters>,
}

/// Full normalized translation bundle produced by a source during installation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, uniffi::Record, Serialize, Deserialize)]
pub struct GenericInstalledTranslation {
    /// Translation metadata and install flags.
    pub translation: GenericTranslation,
    /// All books included in the translation.
    pub books: Vec<GenericBook>,
    /// Chapters grouped by book ID in a UniFFI-friendly sequence.
    pub chapters_by_book: Vec<GenericBookChapters>,
}
