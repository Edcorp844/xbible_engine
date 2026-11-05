use rusqlite::{Connection, params};

use crate::engines::freebible_engine::client::{
    FreeBibleCommentary, FreeBibleDataset, FreeBibleTranslation,
};

// ── Save ────────────────────────────────────────────────────────────

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
            0,
            has_strongs as i32,
            t.license_url,
            t.license_notes,
            t.raw_json,
        ])?;
    }
    Ok(())
}

pub fn save_commentaries(conn: &Connection, items: &[FreeBibleCommentary]) -> rusqlite::Result<()> {
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
            0,
            c.raw_json,
        ])?;
    }
    Ok(())
}

pub fn save_datasets(conn: &Connection, items: &[FreeBibleDataset]) -> rusqlite::Result<()> {
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
            0,
            d.raw_json,
        ])?;
    }
    Ok(())
}

// ── Load ────────────────────────────────────────────────────────────

pub fn load_translations(conn: &Connection) -> rusqlite::Result<Vec<FreeBibleTranslation>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, english_name, short_name, language,
                language_name, language_english_name, text_direction,
                number_of_books, total_number_of_chapters, total_number_of_verses,
                license_url, license_notes, raw_json
         FROM translations",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(FreeBibleTranslation {
            id: r.get(0)?,
            name: r.get(1)?,
            english_name: r.get(2)?,
            short_name: r.get(3)?,
            language: r.get(4)?,
            language_name: r.get(5)?,
            language_english_name: r.get(6)?,
            text_direction: r.get(7)?,
            number_of_books: r.get(8)?,
            total_number_of_chapters: r.get(9)?,
            total_number_of_verses: r.get(10)?,
            license_url: r.get(11)?,
            license_notes: r.get(12)?,
            raw_json: r.get::<_, Option<String>>(13)?.unwrap_or_default(),
        })
    })?;
    rows.collect()
}

pub fn load_commentaries(conn: &Connection) -> rusqlite::Result<Vec<FreeBibleCommentary>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, english_name, language, license_url, license_notes,
                number_of_books, total_number_of_chapters, total_number_of_verses,
                raw_json
         FROM commentaries",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(FreeBibleCommentary {
            id: r.get(0)?,
            name: r.get(1)?,
            english_name: r.get(2)?,
            language: r.get(3)?,
            license_url: r.get(4)?,
            license_notes: r.get(5)?,
            number_of_books: r.get(6)?,
            total_number_of_chapters: r.get(7)?,
            total_number_of_verses: r.get(8)?,
            raw_json: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
        })
    })?;
    rows.collect()
}

pub fn load_datasets(conn: &Connection) -> rusqlite::Result<Vec<FreeBibleDataset>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, english_name, language, license_url, license_notes,
                total_number_of_books, raw_json
         FROM datasets",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(FreeBibleDataset {
            id: r.get(0)?,
            name: r.get(1)?,
            english_name: r.get(2)?,
            language: r.get(3)?,
            license_url: r.get(4)?,
            license_notes: r.get(5)?,
            total_number_of_books: r.get(6)?,
            raw_json: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
        })
    })?;
    rows.collect()
}
