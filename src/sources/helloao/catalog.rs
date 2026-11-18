//! HelloAO catalog views and normalizers (`sources/helloao/catalog.rs`).
//!
//! Belongs to `sources::helloao` rather than `db`, keeping catalog discovery
//! and domain projections scoped to the HelloAO provider.

use crate::sources::helloao::client::{
    HELLOAO_SOURCE_ID, HelloAOClient, HelloCommentaryMeta, HelloDatasetMeta,
};
use crate::sources::helloao::values::{
    BookCount, ChapterCount, Flag, LanguageCode, License, LicenseNotes, LicenseUrl, RawJson,
    TextDirection, VerseCount,
};
use crate::sources::models::{GenericCatalogBible, TextDirection as SourceTextDirection};

// ── Catalog Domain Types ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CatalogTranslation {
    pub id: String,
    pub source_id: String,
    pub name: String,
    pub english_name: String,
    pub short_name: String,
    pub language: LanguageCode,
    pub language_name: Option<String>,
    pub language_english_name: Option<String>,
    pub text_direction: TextDirection,
    pub books: BookCount,
    pub chapters: ChapterCount,
    pub verses: VerseCount,
    pub has_audio: Flag,
    pub has_strongs: Option<Flag>,
    pub license: License,
    pub raw_json: RawJson,
}

#[derive(Debug, Clone)]
pub struct CatalogCommentary {
    pub id: String,
    pub name: String,
    pub english_name: String,
    pub language: LanguageCode,
    pub license: License,
    pub books: BookCount,
    pub chapters: ChapterCount,
    pub verses: VerseCount,
    pub raw_json: RawJson,
}

#[derive(Debug, Clone)]
pub struct CatalogDataset {
    pub id: String,
    pub name: String,
    pub english_name: String,
    pub language: LanguageCode,
    pub license: License,
    pub books: BookCount,
    pub raw_json: RawJson,
}

// ── Conversions ───────────────────────────────────────────────────────────────

impl From<&GenericCatalogBible> for CatalogTranslation {
    fn from(t: &GenericCatalogBible) -> Self {
        let english_name: String = t.english_name.clone().unwrap_or_else(|| t.name.clone());
        let short_name: String = t.short_name.clone().unwrap_or_else(|| t.name.clone());
        let wire_dir: &str = match t.text_direction {
            SourceTextDirection::RightToLeft => "rtl",
            SourceTextDirection::LeftToRight => "ltr",
        };
        let has_strongs: Option<Flag> = t.has_strongs.map(Flag::from_bool);

        Self {
            id: t.id.clone(),
            source_id: t.source_id.clone(),
            name: t.name.clone(),
            english_name,
            short_name,
            language: LanguageCode(t.language_code.clone()),
            language_name: t.language_name.clone(),
            language_english_name: t.language_english_name.clone(),
            text_direction: TextDirection::from_wire(wire_dir),
            books: BookCount::from_raw(t.book_count.unwrap_or(0)),
            chapters: ChapterCount::from_raw(t.chapter_count.unwrap_or(0)),
            verses: VerseCount::from_raw(t.verse_count.unwrap_or(0)),
            has_audio: Flag::from_bool(t.has_audio),
            has_strongs,
            license: License {
                url: t.license_url.clone().map(LicenseUrl),
                notes: t.license_notes.clone().map(LicenseNotes),
            },
            raw_json: RawJson(String::new()),
        }
    }
}

impl From<&HelloCommentaryMeta> for CatalogCommentary {
    fn from(c: &HelloCommentaryMeta) -> Self {
        Self {
            id: c.id.clone(),
            name: c.name.clone(),
            english_name: c.english_name.clone(),
            language: LanguageCode(c.language.clone()),
            license: License {
                url: c.license_url.clone().map(LicenseUrl),
                notes: c.license_notes.clone().map(LicenseNotes),
            },
            books: BookCount::from_raw(c.number_of_books),
            chapters: ChapterCount::from_raw(c.total_number_of_chapters),
            verses: VerseCount::from_raw(c.total_number_of_verses),
            raw_json: RawJson(String::new()),
        }
    }
}

impl From<&HelloDatasetMeta> for CatalogDataset {
    fn from(d: &HelloDatasetMeta) -> Self {
        Self {
            id: d.id.clone(),
            name: d.name.clone(),
            english_name: d.english_name.clone(),
            language: LanguageCode(d.language.clone()),
            license: License {
                url: d.license_url.clone().map(LicenseUrl),
                notes: d.license_notes.clone().map(LicenseNotes),
            },
            books: BookCount::from_raw(d.total_number_of_books),
            raw_json: RawJson(String::new()),
        }
    }
}

impl From<CatalogTranslation> for GenericCatalogBible {
    fn from(d: CatalogTranslation) -> Self {
        let text_direction: SourceTextDirection = match d.text_direction.as_wire() {
            "rtl" | "right-to-left" | "right_to_left" => SourceTextDirection::RightToLeft,
            _ => SourceTextDirection::LeftToRight,
        };
        let has_strongs: Option<bool> = d.has_strongs.map(|f: Flag| f.as_sql_int() != 0);

        Self {
            id: d.id,
            source_id: d.source_id,
            name: d.name,
            english_name: Some(d.english_name),
            short_name: Some(d.short_name),
            language_code: d.language.into_inner(),
            language_name: d.language_name,
            language_english_name: d.language_english_name,
            text_direction,
            book_count: Some(d.books.raw()),
            chapter_count: Some(d.chapters.raw()),
            verse_count: Some(d.verses.raw()),
            has_text: true,
            has_audio: d.has_audio.as_sql_int() != 0,
            has_strongs,
            license_url: d.license.url.map(|u: LicenseUrl| u.0),
            license_notes: d.license.notes.map(|n: LicenseNotes| n.0),
            audio_sources: Vec::new(),
        }
    }
}

// ── Catalog Queries ───────────────────────────────────────────────────────────

pub fn get_translations(client: &HelloAOClient) -> Result<Vec<CatalogTranslation>, String> {
    let raw: Vec<GenericCatalogBible> = client.list_catalog()?;
    let mapped: Vec<CatalogTranslation> = raw.iter().map(CatalogTranslation::from).collect();
    Ok(mapped)
}

pub fn get_commentaries(client: &HelloAOClient) -> Result<Vec<CatalogCommentary>, String> {
    let raw: Vec<HelloCommentaryMeta> = client.commentaries()?;
    let mapped: Vec<CatalogCommentary> = raw.iter().map(CatalogCommentary::from).collect();
    Ok(mapped)
}

pub fn get_datasets(client: &HelloAOClient) -> Result<Vec<CatalogDataset>, String> {
    let raw: Vec<HelloDatasetMeta> = client.datasets()?;
    let mapped: Vec<CatalogDataset> = raw.iter().map(CatalogDataset::from).collect();
    Ok(mapped)
}

pub fn provider_id() -> &'static str {
    HELLOAO_SOURCE_ID
}
