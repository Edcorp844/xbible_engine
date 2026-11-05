use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

pub mod catalog;

const MIGRATIONS: &[M<'_>] = &[
    // ── v1: catalog tables ─────────────────────────────────────────
    M::up(
        r#"
        CREATE TABLE IF NOT EXISTS translations (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            english_name TEXT NOT NULL,
            short_name TEXT NOT NULL,
            language TEXT NOT NULL,
            language_name TEXT,
            language_english_name TEXT,
            text_direction TEXT NOT NULL,
            number_of_books INTEGER NOT NULL,
            total_number_of_chapters INTEGER NOT NULL,
            total_number_of_verses INTEGER NOT NULL,
            installed INTEGER NOT NULL DEFAULT 0,
            has_strongs INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS commentaries (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            english_name TEXT NOT NULL,
            language TEXT NOT NULL,
            license_url TEXT,
            number_of_books INTEGER NOT NULL,
            total_number_of_chapters INTEGER NOT NULL,
            total_number_of_verses INTEGER NOT NULL,
            installed INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS datasets (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            english_name TEXT NOT NULL,
            language TEXT NOT NULL,
            license_url TEXT,
            total_number_of_books INTEGER NOT NULL DEFAULT 0,
            installed INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_translations_language
            ON translations (language);
        CREATE INDEX IF NOT EXISTS idx_commentaries_language
            ON commentaries (language);
        "#,
    ),
    // ── v2: verse content for installed translations ───────────────
    M::up(
        r#"
        CREATE TABLE IF NOT EXISTS books (
            translation_id TEXT NOT NULL,
            id TEXT NOT NULL,
            name TEXT NOT NULL,
            number_of_chapters INTEGER NOT NULL,
            PRIMARY KEY (translation_id, id)
        );

        CREATE TABLE IF NOT EXISTS verses (
            translation_id TEXT NOT NULL,
            book_id TEXT NOT NULL,
            chapter INTEGER NOT NULL,
            verse INTEGER NOT NULL,
            text TEXT NOT NULL,
            PRIMARY KEY (translation_id, book_id, chapter, verse)
        );

        CREATE INDEX IF NOT EXISTS idx_verses_lookup
            ON verses (translation_id, book_id, chapter);
        "#,
    ),
    // ── v3: lexicon + interlinear for word study ───────────────────
    M::up(
        r#"
        CREATE TABLE IF NOT EXISTS lexicon (
            strongs TEXT PRIMARY KEY,
            lemma TEXT,
            transliteration TEXT,
            part_of_speech TEXT,
            gloss TEXT,
            definition TEXT
        );

        CREATE TABLE IF NOT EXISTS interlinear_words (
            translation_id TEXT NOT NULL,
            book_id TEXT NOT NULL,
            chapter INTEGER NOT NULL,
            verse INTEGER NOT NULL,
            position INTEGER NOT NULL,
            word TEXT NOT NULL,
            strongs TEXT,
            morphology TEXT,
            gloss TEXT,
            PRIMARY KEY (translation_id, book_id, chapter, verse, position)
        );

        CREATE INDEX IF NOT EXISTS idx_interlinear_lookup
            ON interlinear_words (translation_id, book_id, chapter, verse);
        "#,
    ),
    // ── v4: full-fidelity structured content ──────────────────────
    M::up(
        r#"
        -- Enrich catalog with license + raw metadata
        ALTER TABLE translations ADD COLUMN license_url TEXT;
        ALTER TABLE translations ADD COLUMN license_notes TEXT;
        ALTER TABLE translations ADD COLUMN raw_json TEXT;

        ALTER TABLE commentaries ADD COLUMN license_notes TEXT;
        ALTER TABLE commentaries ADD COLUMN raw_json TEXT;

        ALTER TABLE datasets ADD COLUMN license_notes TEXT;
        ALTER TABLE datasets ADD COLUMN raw_json TEXT;

        -- Book-level metadata (title, order, common name, audio links)
        CREATE TABLE IF NOT EXISTS book_metadata (
            translation_id TEXT NOT NULL,
            book_id TEXT NOT NULL,
            common_name TEXT,
            title TEXT,
            book_order INTEGER,
            audio_links_json TEXT,
            raw_json TEXT,
            PRIMARY KEY (translation_id, book_id)
        );

        -- Structured chapter items (headings, verses, breaks, subtitles)
        CREATE TABLE IF NOT EXISTS chapter_items (
            translation_id TEXT NOT NULL,
            book_id TEXT NOT NULL,
            chapter INTEGER NOT NULL,
            position INTEGER NOT NULL,
            item_type TEXT NOT NULL,
            verse_number INTEGER,
            text TEXT,
            poem INTEGER NOT NULL DEFAULT 0,
            words_of_jesus INTEGER NOT NULL DEFAULT 0,
            footnote_ids TEXT,
            raw_json TEXT,
            PRIMARY KEY (translation_id, book_id, chapter, position)
        );

        CREATE INDEX IF NOT EXISTS idx_chapter_items_lookup
            ON chapter_items (translation_id, book_id, chapter);

        -- Footnotes per chapter
        CREATE TABLE IF NOT EXISTS footnotes (
            translation_id TEXT NOT NULL,
            book_id TEXT NOT NULL,
            chapter INTEGER NOT NULL,
            note_id TEXT NOT NULL,
            caller TEXT,
            text TEXT,
            reference TEXT,
            raw_json TEXT,
            PRIMARY KEY (translation_id, book_id, chapter, note_id)
        );

        CREATE INDEX IF NOT EXISTS idx_footnotes_lookup
            ON footnotes (translation_id, book_id, chapter);

        -- Per-verse audio timings
        CREATE TABLE IF NOT EXISTS audio_timings (
            translation_id TEXT NOT NULL,
            book_id TEXT NOT NULL,
            chapter INTEGER NOT NULL,
            verse INTEGER NOT NULL,
            start_ms INTEGER,
            end_ms INTEGER,
            PRIMARY KEY (translation_id, book_id, chapter, verse)
        );

        -- Raw chapter JSON — the safety net. Anything the parser doesn't
        -- understand today is preserved here and can be re-parsed later
        -- without re-downloading.
        CREATE TABLE IF NOT EXISTS chapter_raw (
            translation_id TEXT NOT NULL,
            book_id TEXT NOT NULL,
            chapter INTEGER NOT NULL,
            raw_json TEXT NOT NULL,
            PRIMARY KEY (translation_id, book_id, chapter)
        );
        "#,
    ),
];

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    let migrations = Migrations::from_slice(MIGRATIONS);
    migrations.to_latest(&mut conn).map_err(to_sqlite_err)?;
    Ok(conn)
}

fn to_sqlite_err(e: rusqlite_migration::Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_are_valid() {
        let migrations = Migrations::from_slice(MIGRATIONS);
        assert!(migrations.validate().is_ok());
    }

    #[test]
    fn migrations_apply_to_temp_db() {
        let mut conn = Connection::open_in_memory().unwrap();
        let migrations = Migrations::from_slice(MIGRATIONS);
        migrations.to_latest(&mut conn).unwrap();

        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, MIGRATIONS.len() as i32);

        for table in [
            "translations",
            "commentaries",
            "datasets",
            "books",
            "verses",
            "lexicon",
            "interlinear_words",
            "book_metadata",
            "chapter_items",
            "footnotes",
            "audio_timings",
            "chapter_raw",
        ] {
            let exists: i32 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "missing table {}", table);
        }

        for (table, column) in [
            ("translations", "license_url"),
            ("translations", "license_notes"),
            ("translations", "raw_json"),
            ("commentaries", "license_notes"),
            ("commentaries", "raw_json"),
            ("datasets", "license_notes"),
            ("datasets", "raw_json"),
        ] {
            let has: i32 = conn
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
                    [table, column],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(has, 1, "missing column {}.{}", table, column);
        }
    }
}
