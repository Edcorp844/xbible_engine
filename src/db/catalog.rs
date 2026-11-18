use rusqlite::{Connection, Statement, params};

use crate::error::XBibleError;
use crate::sources::{GenericAudioSource, GenericCatalogBible, TextDirection};

pub fn save_translations(
    conn: &Connection,
    translations: &[GenericCatalogBible],
) -> Result<(), XBibleError> {
    let mut stmt: Statement<'_> = conn
        .prepare(
            "INSERT INTO translations (
                id, source_id, name, english_name, short_name,
                language, language_code, language_name, language_english_name,
                text_direction, book_count, chapter_count, verse_count,
                number_of_books, total_number_of_chapters, total_number_of_verses,
                has_text, has_audio, has_strongs, license_url, license_notes,
                audio_sources_json, installed
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5,
                ?6, ?6, ?7, ?8,
                ?9, ?10, ?11, ?12,
                ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17,
                ?18, COALESCE((SELECT installed FROM translations WHERE id = ?1), 0)
             )
             ON CONFLICT(id) DO UPDATE SET
                source_id = excluded.source_id,
                name = excluded.name,
                english_name = excluded.english_name,
                short_name = excluded.short_name,
                language = excluded.language,
                language_code = excluded.language_code,
                language_name = excluded.language_name,
                language_english_name = excluded.language_english_name,
                text_direction = excluded.text_direction,
                book_count = excluded.book_count,
                chapter_count = excluded.chapter_count,
                verse_count = excluded.verse_count,
                number_of_books = excluded.number_of_books,
                total_number_of_chapters = excluded.total_number_of_chapters,
                total_number_of_verses = excluded.total_number_of_verses,
                has_text = excluded.has_text,
                has_audio = excluded.has_audio,
                has_strongs = excluded.has_strongs,
                license_url = excluded.license_url,
                license_notes = excluded.license_notes,
                audio_sources_json = excluded.audio_sources_json",
        )
        .map_err(|e: rusqlite::Error| XBibleError::Generic {
            error_message: format!("prepare save_translations: {e}"),
        })?;

    for t in translations {
        let dir_str: &str = match t.text_direction {
            TextDirection::RightToLeft => "rtl",
            TextDirection::LeftToRight => "ltr",
        };
        let audio_json: String =
            serde_json::to_string(&t.audio_sources).unwrap_or_else(|_| "[]".to_string());

        stmt.execute(params![
            t.id,
            t.source_id,
            t.name,
            t.english_name,
            t.short_name,
            t.language_code,
            t.language_name,
            t.language_english_name,
            dir_str,
            t.book_count,
            t.chapter_count,
            t.verse_count,
            t.has_text as i32,
            t.has_audio as i32,
            t.has_strongs.unwrap_or(false) as i32,
            t.license_url,
            t.license_notes,
            audio_json,
        ])
        .map_err(|e: rusqlite::Error| XBibleError::Generic {
            error_message: format!("insert translation {}: {e}", t.id),
        })?;
    }

    Ok(())
}

pub fn load_translations(conn: &Connection) -> Result<Vec<GenericCatalogBible>, XBibleError> {
    let mut stmt: Statement<'_> = conn
        .prepare(
            "SELECT id, source_id, name, english_name, short_name,
                    COALESCE(NULLIF(language_code, ''), language),
                    language_name, language_english_name,
                    text_direction,
                    COALESCE(NULLIF(book_count, 0), number_of_books),
                    COALESCE(NULLIF(chapter_count, 0), total_number_of_chapters),
                    COALESCE(NULLIF(verse_count, 0), total_number_of_verses),
                    has_text, has_audio, has_strongs, license_url, license_notes,
                    audio_sources_json
             FROM translations",
        )
        .map_err(|e: rusqlite::Error| XBibleError::Generic {
            error_message: format!("prepare load_translations: {e}"),
        })?;

    let rows = stmt
        .query_map([], |row: &rusqlite::Row<'_>| {
            let dir_raw: String = row.get::<_, Option<String>>(8)?.unwrap_or_default();
            let text_direction: TextDirection = match dir_raw.to_ascii_lowercase().as_str() {
                "rtl" | "right-to-left" | "right_to_left" => TextDirection::RightToLeft,
                _ => TextDirection::LeftToRight,
            };
            let has_text: bool = row.get::<_, i32>(12).unwrap_or(1) != 0;
            let has_audio: bool = row.get::<_, i32>(13).unwrap_or(0) != 0;
            let has_strongs: Option<bool> = row.get::<_, Option<i32>>(14)?.map(|v: i32| v != 0);
            let audio_json: Option<String> = row.get(17)?;
            let audio_sources: Vec<GenericAudioSource> = audio_json
                .and_then(|s: String| serde_json::from_str::<Vec<GenericAudioSource>>(&s).ok())
                .unwrap_or_default();

            Ok(GenericCatalogBible {
                id: row.get(0)?,
                source_id: row
                    .get::<_, Option<String>>(1)?
                    .unwrap_or_else(|| "helloao".to_string()),
                name: row.get(2)?,
                english_name: row.get(3)?,
                short_name: row.get(4)?,
                language_code: row.get(5)?,
                language_name: row.get(6)?,
                language_english_name: row.get(7)?,
                text_direction,
                book_count: row.get(9)?,
                chapter_count: row.get(10)?,
                verse_count: row.get(11)?,
                has_text,
                has_audio,
                has_strongs,
                license_url: row.get(15)?,
                license_notes: row.get(16)?,
                audio_sources,
            })
        })
        .map_err(|e: rusqlite::Error| XBibleError::Generic {
            error_message: format!("query load_translations: {e}"),
        })?;

    let mut out: Vec<GenericCatalogBible> = Vec::new();
    for r in rows {
        out.push(r.map_err(|e: rusqlite::Error| XBibleError::Generic {
            error_message: format!("row load_translations: {e}"),
        })?);
    }
    Ok(out)
}
