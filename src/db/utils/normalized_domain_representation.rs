//! Generic, source-agnostic translation structs for database storage
//! and cross-language FFI transfer via UniFFI.
//!
//! Sources (such as HelloAO or future providers) normalize upstream API
//! payloads into these types so the store, reader, search engine, and
//! UI remain insulated from wire-format differences[cite: 6, 8, 12, 14].
//!
//! UniFFI compatibility notes:
//! - Every exposed struct/enum derives `uniffi::Record` or `uniffi::Enum`
//!   using owned types (`String`, `Vec`, `Option`, `bool`, `i32`, `i64`)[cite: 8, 12, 14].
//! - Nested chapter collections use `Vec<GenericBookChapters>` rather than
//!   `HashMap<String, Vec<GenericChapter>>` so FFI bindings remain uniform
//!   and predictable across targets.

/// Reading flow direction for a translation's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TextDirection {
    /// Left-to-right script flow (for example, Latin or Greek).
    LeftToRight,
    /// Right-to-right script flow (for example, Arabic or Hebrew).
    RightToLeft,
}

/// Canonical book grouping used to organize and filter books per canon.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum CanonicalSection {
    /// Standard Old Testament books.
    OldTestament,
    /// Standard New Testament books.
    NewTestament,
    /// Deuterocanonical / apocryphal books.
    Deuterocanon,
    /// Additional books specific to Orthodox canons.
    OrthodoxExtra,
    /// Additional books specific to Ethiopian canons.
    EthiopianExtra,
    /// Unclassified or pre-migration section when no canonical bucket is known.
    Unknown,
    /// Provider- or edition-specific section label when none of the standard
    /// canonical buckets fit.
    Custom(String),
}

/// Semantic role of an inline span within a chapter block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Enum)]
pub enum SpanKind {
    /// Plain prose or narrative text run.
    #[default]
    Text,
    /// Individual word token (useful for lexical, Strong's, or morphology data).
    Word,
    /// Anchor or marker referencing a chapter footnote.
    FootnoteRef,
    /// Anchor or marker referencing a cross-reference.
    CrossRef,
    /// Quoted speech or direct quotation segment.
    Quote,
    /// Any other source-specific inline span kind.
    Other,
}

/// Visual and typographic styling flags applied to a span.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct SpanFormatting {
    /// True when rendered in red-letter / words-of-Jesus styling.
    pub is_words_of_jesus: bool,
    /// True when italicized (such as supplied words or emphasis).
    pub is_italic: bool,
    /// True when rendered in bold weight.
    pub is_bold: bool,
    /// True when rendered in small caps.
    pub is_small_caps: bool,
    /// True when underlined.
    pub is_underline: bool,
    /// True when formatted as poetic/verse line styling.
    pub is_poetry: bool,
}

/// Original-language and lexical annotations attached to a span when available.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct LexicalInfo {
    /// Strong's concordance identifier (for example, `"H430"` or `"G26"`).
    pub strongs: Option<String>,
    /// Dictionary / base lemma form.
    pub lemma: Option<String>,
    /// Latin or phonetic transliteration of the original form.
    pub transliteration: Option<String>,
    /// Grammatical part-of-speech tag.
    pub part_of_speech: Option<String>,
    /// Detailed morphological parsing code or description.
    pub morphology: Option<String>,
    /// Short gloss or translation equivalent.
    pub gloss: Option<String>,
}

/// Normalized inline text segment inside a `GenericChapterItem`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericTextSpan {
    /// Zero- or one-based ordering index within the parent item.
    pub position: i32,
    /// Role of this span within the block.
    pub kind: SpanKind,
    /// Visible text content of the span.
    pub text: String,
    /// Typographic and rendering flags for this span.
    pub formatting: SpanFormatting,
    /// Optional lexical/Strong's/morphological metadata for word-level sources.
    pub lexical: Option<LexicalInfo>,
    /// Identifiers of footnotes linked from this span.
    pub footnote_ids: Vec<String>,
    /// Identifiers of cross-references linked from this span.
    pub cross_ref_ids: Vec<String>,
    /// Optional ISO/BCP-47 language code when this span differs from the book/chapter.
    pub language_code: Option<String>,
    /// Optional source-specific payload retained for inspection or re-parsing.
    pub raw_json: Option<String>,
}

/// Catalog and capability record for a Bible translation stored in the DB.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericTranslation {
    /// Translation identifier (unique within the source or namespaced key).
    pub id: String,
    /// Identifier of the source provider supplying this translation (for example, `"helloao"`).
    pub source_id: String,
    /// Native or primary display name of the translation.
    pub name: String,
    /// English display name of the translation.
    pub english_name: String,
    /// Short abbreviation or sigla (for example, `"ESV"`, `"KJV"`, `"BSB"`).
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

/// Book-level structure and metadata persisted for an installed translation.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericBook {
    /// Owning translation identifier.
    pub translation_id: String,
    /// Canonical or source book code (for example, `"GEN"`, `"JHN"`, `"REV"`).
    pub id: String,
    /// Standard book name for display.
    pub name: String,
    /// Common or alternate book name when provided.
    pub common_name: Option<String>,
    /// Full or formal book title when provided.
    pub title: Option<String>,
    /// Sort/sequence order within the translation.
    pub book_order: Option<i32>,
    /// Canonical section classification used for canon filtering and grouping.
    pub section: CanonicalSection,
    /// Number of chapters in this book.
    pub number_of_chapters: i32,
    /// Optional JSON-encoded audio resource links for the book.
    pub audio_links_json: Option<String>,
    /// Optional raw JSON metadata preserved from the source.
    pub raw_json: Option<String>,
}

/// Ordered structural element within a chapter (heading, verse block, break, etc.).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericChapterItem {
    /// Ordering position of the item within the chapter.
    pub position: i32,
    /// Normalized item category (for example, `"verse"`, `"heading"`, `"line_break"`).
    pub item_type: String,
    /// Verse number when the item corresponds to a specific verse.
    pub verse_number: Option<i32>,
    /// Plain-text representation of the item when applicable.
    pub text: Option<String>,
    /// True if the block represents poetic lines.
    pub is_poem: bool,
    /// True if the block contains words of Jesus.
    pub is_words_of_jesus: bool,
    /// Footnote identifiers associated with this item.
    pub footnote_ids: Vec<String>,
    /// Structured inline spans making up this item.
    pub spans: Vec<GenericTextSpan>,
    /// Optional source-specific JSON retained for backward compatibility or re-parsing.
    pub raw_json: Option<String>,
}

/// Normalized verse record used for direct verse lookup and full-text search indexing.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericVerse {
    /// Book identifier containing this verse.
    pub book_id: String,
    /// One-based chapter number.
    pub chapter: i32,
    /// One-based verse number.
    pub verse: i32,
    /// Merged/normalized verse text.
    pub text: String,
}

/// Footnote entry associated with a chapter.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericFootnote {
    /// Footnote identifier referenced by chapter items or spans.
    pub note_id: String,
    /// Optional marker or caller symbol (for example, `"a"` or `"*"`).
    pub caller: Option<String>,
    /// Footnote body text.
    pub text: Option<String>,
    /// Optional scripture or external reference cited in the note.
    pub reference: Option<String>,
    /// Optional raw JSON payload from the source.
    pub raw_json: Option<String>,
}

/// Audio timing alignment for a verse within an audio-backed chapter.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericAudioTiming {
    /// Verse number within the chapter.
    pub verse: i32,
    /// Start offset in milliseconds from the beginning of the chapter audio.
    pub start_ms: Option<i64>,
    /// End offset in milliseconds from the beginning of the chapter audio.
    pub end_ms: Option<i64>,
}

/// Complete normalized representation of a single chapter ready for DB persistence.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericChapter {
    /// Owning translation identifier.
    pub translation_id: String,
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
    /// Per-verse audio timestamps for this chapter.
    pub audio_timings: Vec<GenericAudioTiming>,
    /// Raw chapter JSON preserved as a safety net so future parsers can re-extract fields.
    pub raw_json: String,
}

/// Pairs a book identifier with all normalized chapters belonging to that book.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericBookChapters {
    /// Book identifier (`GenericBook::id`).
    pub book_id: String,
    /// Chapters for this book in chapter order.
    pub chapters: Vec<GenericChapter>,
}

/// Full normalized translation bundle produced by a source during installation.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GenericInstalledTranslation {
    /// Translation metadata and install flags.
    pub translation: GenericTranslation,
    /// All books included in the translation.
    pub books: Vec<GenericBook>,
    /// Chapters grouped by book ID in a UniFFI-friendly sequence.
    pub chapters_by_book: Vec<GenericBookChapters>,
}
