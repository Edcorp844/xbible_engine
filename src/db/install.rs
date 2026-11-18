//! Generic SQLite persistence routines for installed Bible content (`db/install.rs`).
//!
//! Consumes normalized `Generic*` types produced by any `BibleSource` so
//! database writes, verse merging, and `installed` flags remain uniform
//! across providers without `db/` depending on HTTP client wire types.

use std::collections::BTreeMap;

use log::info;
use rusqlite::{Connection, Statement, params};

use crate::{
    error::XBibleError,
    sources::models::{
        CanonicalSection, GenericAudioTiming, GenericBook, GenericChapter, GenericChapterItem,
        GenericFootnote, GenericInstalledBible, GenericVerse,
    },
};

const CONTENT_TABLES: &[&str] = &[
    "verses",
    "books",
    "book_metadata",
    "chapter_items",
    "footnotes",
    "audio_timings",
    "chapter_raw",
];

// ── Canonical book section ────────────────────────────────────────────────────

pub(crate) fn section_to_wire(section: &CanonicalSection) -> String {
    match section {
        CanonicalSection::OldTestament => "ot".to_string(),
        CanonicalSection::NewTestament => "nt".to_string(),
        CanonicalSection::Deuterocanon => "deutero".to_string(),
        CanonicalSection::OrthodoxExtra => "orthodox_extra".to_string(),
        CanonicalSection::EthiopianExtra => "ethiopian_extra".to_string(),
        CanonicalSection::Unknown => "unknown".to_string(),
        CanonicalSection::Custom(label) => label.clone(),
    }
}

pub(crate) fn section_for_book(book_id: &str, is_apocryphal: bool) -> &'static str {
    let id: String = book_id.to_uppercase();

    const ETHIOPIAN_EXTRA: &[&str] = &["ENO", "JUB", "1MQ", "2MQ", "3MQ", "4BA"];
    const ORTHODOX_EXTRA: &[&str] = &["3MA", "4MA", "DAG", "ADE"];

    if ETHIOPIAN_EXTRA.contains(&id.as_str()) {
        return "ethiopian_extra";
    }
    if ORTHODOX_EXTRA.contains(&id.as_str()) {
        return "orthodox_extra";
    }

    if !is_apocryphal {
        const NT: &[&str] = &[
            "MAT", "MRK", "LUK", "JHN", "ACT", "ROM", "1CO", "2CO", "GAL", "EPH", "PHP", "COL",
            "1TH", "2TH", "1TI", "2TI", "TIT", "PHM", "HEB", "JAS", "1PE", "2PE", "1JN", "2JN",
            "3JN", "JUD", "REV",
        ];
        if NT.contains(&id.as_str()) {
            return "nt";
        }
        return "ot";
    }

    "deutero"
}

// ── Verse merging ─────────────────────────────────────────────────────────────

fn merge_verse_rows(verses: &[GenericVerse]) -> BTreeMap<i32, String> {
    let mut merged: BTreeMap<i32, String> = BTreeMap::new();
    for v in verses {
        merged
            .entry(v.verse)
            .and_modify(|existing: &mut String| {
                if !existing.is_empty() && !v.text.is_empty() {
                    existing.push(' ');
                }
                existing.push_str(&v.text);
            })
            .or_insert_with(|| v.text.clone());
    }
    merged
}

// ── Translation install & uninstall primitives ──────────────────────────────

pub(crate) fn wipe_translation(conn: &Connection, translation_id: &str) -> Result<(), XBibleError> {
    for table in CONTENT_TABLES {
        let sql: String = format!("DELETE FROM {table} WHERE translation_id = ?1");
        conn.execute(&sql, params![translation_id])
            .map_err(|e: rusqlite::Error| XBibleError::Generic {
                error_message: e.to_string(),
            })?;
    }
    Ok(())
}

pub(crate) fn begin_install(conn: &Connection, translation_id: &str) -> Result<(), XBibleError> {
    wipe_translation(conn, translation_id)?;
    conn.execute(
        "UPDATE translations SET installed = 0 WHERE id = ?1",
        params![translation_id],
    )
    .map_err(|e: rusqlite::Error| XBibleError::Generic {
        error_message: format!("clear installed flag: {e}"),
    })?;
    Ok(())
}

pub fn persist_book_chapters(
    conn: &Connection,
    translation_id: &str,
    book: &GenericBook,
    chapters: &[GenericChapter],
) -> Result<(), XBibleError> {
    conn.execute(
        "INSERT OR REPLACE INTO books (translation_id, id, name, number_of_chapters)
         VALUES (?1, ?2, ?3, ?4)",
        params![translation_id, book.id, book.name, book.number_of_chapters],
    )
    .map_err(|e: rusqlite::Error| XBibleError::Generic {
        error_message: format!("books insert ({}): {e}", book.id),
    })?;

    let is_apocryphal: bool = matches!(book.section, CanonicalSection::Deuterocanon);
    let section_str: String = match &book.section {
        CanonicalSection::Unknown => section_for_book(&book.id, is_apocryphal).to_string(),
        other => section_to_wire(other),
    };

    let audio_links_json: Option<String> = if book.audio_sources.is_empty() {
        None
    } else {
        serde_json::to_string(&book.audio_sources).ok()
    };

    conn.execute(
        "INSERT OR REPLACE INTO book_metadata (
            translation_id, book_id, common_name, title,
            book_order, audio_links_json, raw_json, section
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            translation_id,
            book.id,
            book.common_name,
            book.title,
            book.book_order,
            audio_links_json,
            Option::<String>::None,
            section_str,
        ],
    )
    .map_err(|e: rusqlite::Error| XBibleError::Generic {
        error_message: format!("book_metadata insert ({}): {e}", book.id),
    })?;

    let mut verse_stmt: Statement<'_> = conn
        .prepare(
            "INSERT OR REPLACE INTO verses (translation_id, book_id, chapter, verse, text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .map_err(|e: rusqlite::Error| XBibleError::Generic {
            error_message: format!("prepare verses: {e}"),
        })?;
    let mut item_stmt: Statement<'_> = conn
        .prepare(
            "INSERT OR REPLACE INTO chapter_items (
                translation_id, book_id, chapter, position, item_type,
                verse_number, text, poem, words_of_jesus, footnote_ids, raw_json
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        )
        .map_err(|e: rusqlite::Error| XBibleError::Generic {
            error_message: format!("prepare chapter_items: {e}"),
        })?;
    let mut footnote_stmt: Statement<'_> = conn
        .prepare(
            "INSERT OR REPLACE INTO footnotes (
                translation_id, book_id, chapter, note_id,
                caller, text, reference, raw_json
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        )
        .map_err(|e: rusqlite::Error| XBibleError::Generic {
            error_message: format!("prepare footnotes: {e}"),
        })?;
    let mut timing_stmt: Statement<'_> = conn
        .prepare(
            "INSERT OR REPLACE INTO audio_timings (
                translation_id, book_id, chapter, verse, start_ms, end_ms
             ) VALUES (?1,?2,?3,?4,?5,?6)",
        )
        .map_err(|e: rusqlite::Error| XBibleError::Generic {
            error_message: format!("prepare audio_timings: {e}"),
        })?;
    let mut raw_stmt: Statement<'_> = conn
        .prepare(
            "INSERT OR REPLACE INTO chapter_raw (translation_id, book_id, chapter, raw_json)
             VALUES (?1,?2,?3,?4)",
        )
        .map_err(|e: rusqlite::Error| XBibleError::Generic {
            error_message: format!("prepare chapter_raw: {e}"),
        })?;

    for chapter in chapters {
        let merged: BTreeMap<i32, String> = merge_verse_rows(&chapter.verses);
        for (number, text) in &merged {
            verse_stmt
                .execute(params![
                    translation_id,
                    book.id,
                    chapter.chapter_number,
                    number,
                    text
                ])
                .map_err(|e: rusqlite::Error| XBibleError::Generic {
                    error_message: format!(
                        "verses insert {} {}:{}: {e}",
                        book.id, chapter.chapter_number, number
                    ),
                })?;
        }

        for item in &chapter.items {
            let item: &GenericChapterItem = item;
            let footnote_ids: Option<String> = if item.footnote_ids.is_empty() {
                None
            } else {
                Some(item.footnote_ids.join(","))
            };
            let spans_json: String = serde_json::to_string(&item.spans).unwrap_or_default();
            item_stmt
                .execute(params![
                    translation_id,
                    book.id,
                    chapter.chapter_number,
                    item.position,
                    item.item_type,
                    item.verse_number,
                    item.text,
                    item.is_poem as i32,
                    item.is_words_of_jesus as i32,
                    footnote_ids,
                    spans_json,
                ])
                .map_err(|e: rusqlite::Error| XBibleError::Generic {
                    error_message: format!(
                        "chapter_items insert {} {}:{}: {e}",
                        book.id, chapter.chapter_number, item.position
                    ),
                })?;
        }

        for note in &chapter.footnotes {
            let note: &GenericFootnote = note;
            footnote_stmt
                .execute(params![
                    translation_id,
                    book.id,
                    chapter.chapter_number,
                    note.note_id,
                    note.caller,
                    note.text,
                    note.reference,
                    "{}",
                ])
                .map_err(|e: rusqlite::Error| XBibleError::Generic {
                    error_message: format!(
                        "footnotes insert {} {}:{}: {e}",
                        book.id, chapter.chapter_number, note.note_id
                    ),
                })?;
        }

        for timing in &chapter.audio_timings {
            let timing: &GenericAudioTiming = timing;
            timing_stmt
                .execute(params![
                    translation_id,
                    book.id,
                    chapter.chapter_number,
                    timing.verse,
                    timing.start_ms,
                    timing.end_ms,
                ])
                .map_err(|e: rusqlite::Error| XBibleError::Generic {
                    error_message: format!(
                        "audio_timings insert {} {}:{}: {e}",
                        book.id, chapter.chapter_number, timing.verse
                    ),
                })?;
        }

        let chapter_snapshot: String =
            serde_json::to_string(&chapter.items).unwrap_or_else(|_| "[]".to_string());
        raw_stmt
            .execute(params![
                translation_id,
                book.id,
                chapter.chapter_number,
                chapter_snapshot,
            ])
            .map_err(|e: rusqlite::Error| XBibleError::Generic {
                error_message: format!(
                    "chapter_raw insert {} {}: {e}",
                    book.id, chapter.chapter_number
                ),
            })?;
    }

    Ok(())
}

/// Persists a complete `GenericInstalledBible` bundle inside a transaction and
/// marks the translation as installed.
pub fn persist_installed_bible(
    conn: &Connection,
    bundle: &GenericInstalledBible,
) -> Result<(), XBibleError> {
    let translation_id: &str = &bundle.catalog.id;
    begin_install(conn, translation_id)?;

    for book in &bundle.books {
        let chapters: &[GenericChapter] = bundle
            .chapters_by_book
            .iter()
            .find(|group| group.book_id == book.id)
            .map(|group| group.chapters.as_slice())
            .unwrap_or(&[]);
        persist_book_chapters(conn, translation_id, book, chapters)?;
    }

    mark_installed(conn, translation_id)?;
    Ok(())
}

pub(crate) fn mark_installed(conn: &Connection, translation_id: &str) -> Result<(), XBibleError> {
    let n: usize = conn
        .execute(
            "UPDATE translations SET installed = 1 WHERE id = ?1",
            params![translation_id],
        )
        .map_err(|e: rusqlite::Error| XBibleError::Generic {
            error_message: format!("mark installed: {e}"),
        })?;
    if n == 0 {
        return Err(XBibleError::Generic {
            error_message: format!(
                "translation {translation_id} not in catalog — refresh source catalog first"
            ),
        });
    }
    Ok(())
}

pub(crate) fn uninstall_translation(
    conn: &Connection,
    translation_id: &str,
) -> Result<(), XBibleError> {
    wipe_translation(conn, translation_id)?;
    let n: usize = conn
        .execute(
            "UPDATE translations SET installed = 0 WHERE id = ?1",
            params![translation_id],
        )
        .map_err(|e: rusqlite::Error| XBibleError::Generic {
            error_message: format!("clear installed flag: {e}"),
        })?;
    if n == 0 {
        return Err(XBibleError::Generic {
            error_message: format!("translation {translation_id} not in catalog"),
        });
    }
    info!("uninstalled {}", translation_id);
    Ok(())
}
