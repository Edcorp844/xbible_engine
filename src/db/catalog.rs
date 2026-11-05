// freebible_engine/db/catalog.rs

use rusqlite::{Connection, params};

use crate::engines::freebible_engine::client::{
    FreeBibleCommentary, FreeBibleDataset, FreeBibleTranslation,
};
use crate::engines::freebible_engine::values::{
    BookCount, ChapterCount, Flag, LanguageCode, License, LicenseNotes, LicenseUrl,
    RawJson, TextDirection, VerseCount,
};

// ── Domain types ────────────────────────────────────────────────

/// Domain view of a translation. Newtypes everywhere so counts,
/// language codes, and license fields can't be swapped at call sites.
#[derive(Debug, Clone)]
pub struct DomainTranslation {
    pub id: String,
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
    pub license: License,
    pub raw_json: RawJson,
}

#[derive(Debug, Clone)]
pub struct DomainCommentary {
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
pub struct DomainDataset {
    pub id: String,
    pub name: String,
    pub english_name: String,
    pub language: LanguageCode,
    pub license: License,
    pub books: BookCount,
    pub raw_json: RawJson,
}

// ── Client record → Domain ──────────────────────────────────────

impl From<&FreeBibleTranslation> for DomainTranslation {
    fn from(t: &FreeBibleTranslation) -> Self {
        Self {
            id: t.id.clone(),
            name: t.name.clone(),
            english_name: t.english_name.clone(),
            short_name: t.short_name.clone(),
            language: LanguageCode(t.language.clone()),
            language_name: t.language_name.clone(),
            language_english_name: t.language_english_name.clone(),
            text_direction: TextDirection::from_wire(&t.text_direction),
            books: BookCount::from_raw(t.number_of_books),
            chapters: ChapterCount::from_raw(t.total_number_of_chapters),
            verses: VerseCount::from_raw(t.total_number_of_verses),
            license: License {
                url: t.license_url.clone().map(LicenseUrl),
                notes: t.license_notes.clone().map(LicenseNotes),
            },
            raw_json: RawJson(t.raw_json.clone()),
        }
    }
}

impl From<&FreeBibleCommentary> for DomainCommentary {
    fn from(c: &FreeBibleCommentary) -> Self {
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
            raw_json: RawJson(c.raw_json.clone()),
        }
    }
}

impl From<&FreeBibleDataset> for DomainDataset {
    fn from(d: &FreeBibleDataset) -> Self {
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
            raw_json: RawJson(d.raw_json.clone()),
        }
    }
}

// ── Domain → Client record (for the FFI boundary) ──────────────

impl From<DomainTranslation> for FreeBibleTranslation {
    fn from(d: DomainTranslation) -> Self {
        Self {
            id: d.id,
            name: d.name,
            english_name: d.english_name,
            short_name: d.short_name,
            language: d.language.into_inner(),
            language_name: d.language_name,
            language_english_name: d.language_english_name,
            text_direction: d.text_direction.as_wire().to_string(),
            number_of_books: d.books.raw(),
            total_number_of_chapters: d.chapters.raw(),
            total_number_of_verses: d.verses.raw(),
            license_url: d.license.url.map(|u| u.0),
            license_notes: d.license.notes.map(|n| n.0),
            raw_json: d.raw_json.0,
        }
    }
}

impl From<DomainCommentary> for FreeBibleCommentary {
    fn from(d: DomainCommentary) -> Self {
        Self {
            id: d.id,
            name: d.name,
            english_name: d.english_name,
            language: d.language.into_inner(),
            license_url: d.license.url.map(|u| u.0),
            license_notes: d.license.notes.map(|n| n.0),
            number_of_books: d.books.raw(),
            total_number_of_chapters: d.chapters.raw(),
            total_number_of_verses: d.verses.raw(),
            raw_json: d.raw_json.0,
        }
    }
}

impl From<DomainDataset> for FreeBibleDataset {
    fn from(d: DomainDataset) -> Self {
        Self {
            id: d.id,
            name: d.name,
            english_name: d.english_name,
            language: d.language.into_inner(),
            license_url: d.license.url.map(|u| u.0),
            license_notes: d.license.notes.map(|n| n.0),
            total_number_of_books: d.books.raw(),
            raw_json: d.raw_json.0,
        }
    }
}

// ── Save ────────────────────────────────────────────────────────

pub fn save_translations(
    conn: &Connection,
    items: &[FreeBibleTranslation],
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO translations (
            id, name, english_name, short_name, language,
            language_name, language_english_name, text_direction,
            number_of_books, total_number_of_chapters, total_number_of_verses,
            installed, has_strongs,
            license_url, license_notes, raw_json
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
    )?;
    for t in items {
        let has_strongs = matches!(t.id.as_str(), "BSB") || t.id.starts_with("eng_kjv");

        stmt.execute(params![
            t.id,
            t.name,
            t.english_name,
            t.short_name,
            t.language,
            t.language_name,
            t.language_english_name,
            t.text_direction,
            t.number_of_books,
            t.total_number_of_chapters,
            t.total_number_of_verses,
            Flag::Off.as_sql_int(),
            Flag::from_bool(has_strongs).as_sql_int(),
            t.license_url,
            t.license_notes,
            t.raw_json,
        ])?;
    }
    Ok(())
}

pub fn save_commentaries(
    conn: &Connection,
    items: &[FreeBibleCommentary],
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO commentaries (
            id, name, english_name, language, license_url, license_notes,
            number_of_books, total_number_of_chapters, total_number_of_verses,
            installed, raw_json
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
    )?;
    for c in items {
        stmt.execute(params![
            c.id,
            c.name,
            c.english_name,
            c.language,
            c.license_url,
            c.license_notes,
            c.number_of_books,
            c.total_number_of_chapters,
            c.total_number_of_verses,
            Flag::Off.as_sql_int(),
            c.raw_json,
        ])?;
    }
    Ok(())
}

pub fn save_datasets(
    conn: &Connection,
    items: &[FreeBibleDataset],
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO datasets (
            id, name, english_name, language, license_url, license_notes,
            total_number_of_books, installed, raw_json
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
    )?;
    for d in items {
        stmt.execute(params![
            d.id,
            d.name,
            d.english_name,
            d.language,
            d.license_url,
            d.license_notes,
            d.total_number_of_books,
            Flag::Off.as_sql_int(),
            d.raw_json,
        ])?;
    }
    Ok(())
}

// ── Load ────────────────────────────────────────────────────────

pub fn load_translations(conn: &Connection) -> rusqlite::Result<Vec<DomainTranslation>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, english_name, short_name, language,
                language_name, language_english_name, text_direction,
                number_of_books, total_number_of_chapters, total_number_of_verses,
                license_url, license_notes, raw_json
         FROM translations",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(DomainTranslation {
            id: r.get(0)?,
            name: r.get(1)?,
            english_name: r.get(2)?,
            short_name: r.get(3)?,
            language: LanguageCode(r.get(4)?),
            language_name: r.get(5)?,
            language_english_name: r.get(6)?,
            text_direction: TextDirection::from_wire(&r.get::<_, String>(7)?),
            books: BookCount::from_raw(r.get(8)?),
            chapters: ChapterCount::from_raw(r.get(9)?),
            verses: VerseCount::from_raw(r.get(10)?),
            license: License {
                url: r.get::<_, Option<String>>(11)?.map(LicenseUrl),
                notes: r.get::<_, Option<String>>(12)?.map(LicenseNotes),
            },
            raw_json: RawJson(r.get::<_, Option<String>>(13)?.unwrap_or_default()),
        })
    })?;
    rows.collect()
}

pub fn load_commentaries(conn: &Connection) -> rusqlite::Result<Vec<DomainCommentary>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, english_name, language, license_url, license_notes,
                number_of_books, total_number_of_chapters, total_number_of_verses,
                raw_json
         FROM commentaries",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(DomainCommentary {
            id: r.get(0)?,
            name: r.get(1)?,
            english_name: r.get(2)?,
            language: LanguageCode(r.get(3)?),
            license: License {
                url: r.get::<_, Option<String>>(4)?.map(LicenseUrl),
                notes: r.get::<_, Option<String>>(5)?.map(LicenseNotes),
            },
            books: BookCount::from_raw(r.get(6)?),
            chapters: ChapterCount::from_raw(r.get(7)?),
            verses: VerseCount::from_raw(r.get(8)?),
            raw_json: RawJson(r.get::<_, Option<String>>(9)?.unwrap_or_default()),
        })
    })?;
    rows.collect()
}

pub fn load_datasets(conn: &Connection) -> rusqlite::Result<Vec<DomainDataset>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, english_name, language, license_url, license_notes,
                total_number_of_books, raw_json
         FROM datasets",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(DomainDataset {
            id: r.get(0)?,
            name: r.get(1)?,
            english_name: r.get(2)?,
            language: LanguageCode(r.get(3)?),
            license: License {
                url: r.get::<_, Option<String>>(4)?.map(LicenseUrl),
                notes: r.get::<_, Option<String>>(5)?.map(LicenseNotes),
            },
            books: BookCount::from_raw(r.get(6)?),
            raw_json: RawJson(r.get::<_, Option<String>>(7)?.unwrap_or_default()),
        })
    })?;
    rows.collect()
}