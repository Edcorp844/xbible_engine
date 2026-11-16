//! Integration tests for the shared SQLite layer.
//!
//! These live in `tests/` (a separate crate), so they reach the
//! engine through `xbible_engine::` exactly as an external consumer
//! would. That requires `db::open` and `install::persist_book` to be
//! `pub` rather than `pub(crate)` — both are, so the tests exercise
//! the same code path the engine uses without going through the
//! network.

use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::Connection;

// ── Temp file guard ─────────────────────────────────────────────
//
// Every test gets a unique path (name + pid + counter) so parallel
// `cargo test` threads never collide, and a `Drop` guard so a failed
// assertion still removes the main DB and its WAL/SHM siblings.
// Without the guard, a panic leaks three files into /tmp per failing
// run; on CI that fills the disk over months.

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TempDb {
    path: String,
}

impl TempDb {
    fn new(name: &str) -> Self {
        let n = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "xbible_test_{}_{}_{}.db",
            name,
            std::process::id(),
            n
        ));
        let path = p.to_string_lossy().into_owned();
        let _ = Self::remove(&path);
        Self { path }
    }

    fn path(&self) -> &str {
        &self.path
    }

    fn remove(path: &str) {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(format!("{path}-wal"));
        let _ = std::fs::remove_file(format!("{path}-shm"));
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        Self::remove(&self.path);
    }
}

// ── Helpers ─────────────────────────────────────────────────────

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |r| r.get::<_, i32>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
        [table, column],
        |r| r.get::<_, i32>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

fn user_version(conn: &Connection) -> i32 {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0)
}

// ── Schema ──────────────────────────────────────────────────────

#[test]
fn db_creates_tables_and_sets_version() {
    let db = TempDb::new("schema");
    let conn = xbible_engine::db::open(db.path()).expect("open failed");

    let v = user_version(&conn);
    assert!(v >= 1, "expected at least one migration, got {v}");

    // Every table the migrations create must exist. This list must
    // stay in sync with `migrations::MIGRATIONS`; if you add a
    // migration, add its tables here in the same PR.
    for table in [
        // v1: catalog
        "translations",
        "commentaries",
        "datasets",
        // v2: verse content
        "books",
        "verses",
        // v3: word study
        "lexicon",
        "interlinear_words",
        // v4: full-fidelity structure
        "book_metadata",
        "chapter_items",
        "footnotes",
        "audio_timings",
        "chapter_raw",
        // v5: Theographic entity graph
        "people",
        "events",
        "places",
        "people_groups",
        "entity_refs",
        "entity_verse_refs",
        "entity_install_state",
        // v6: commentary content
        "commentary_books",
        "commentary_chapters",
        // v7: FTS5 verse index
        "verses_fts",
    ] {
        assert!(table_exists(&conn, table), "missing table {table}");
    }

    // Columns added by v4 via ALTER TABLE must be present. If a later
    // migration adds columns, add them here.
    for (table, column) in [
        ("translations", "license_url"),
        ("translations", "license_notes"),
        ("translations", "raw_json"),
        ("commentaries", "license_notes"),
        ("commentaries", "raw_json"),
        ("datasets", "license_notes"),
        ("datasets", "raw_json"),
    ] {
        assert!(
            column_exists(&conn, table, column),
            "missing column {table}.{column}"
        );
    }

    drop(conn);

    // Reopening must not re-run migrations or change the version.
    let conn2 = xbible_engine::db::open(db.path()).expect("reopen failed");
    assert_eq!(
        user_version(&conn2),
        v,
        "reopen changed user_version — migrations re-ran"
    );
    drop(conn2);
}

// ── Pragmas ─────────────────────────────────────────────────────

#[test]
fn wal_mode_is_enabled() {
    let db = TempDb::new("wal");
    let conn = xbible_engine::db::open(db.path()).unwrap();

    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal");

    drop(conn);
}

#[test]
fn busy_timeout_and_foreign_keys_are_set() {
    // These are the two pragmas whose absence produces real incidents
    // on Android (SQLITE_BUSY during concurrent writes) and on every
    // platform (silent FK violations). Assert them once, here, so a
    // refactor of `open` cannot quietly drop them.
    let db = TempDb::new("pragmas");
    let conn = xbible_engine::db::open(db.path()).unwrap();

    let timeout: i64 = conn
        .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
        .unwrap();
    assert!(timeout > 0, "busy_timeout must be set, got {timeout}");

    let fk: i32 = conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!(fk, 1, "foreign_keys must be ON");

    drop(conn);
}

// ── Content round-trip ──────────────────────────────────────────

#[test]
fn chapter_items_survive_insert_and_query() {
    let db = TempDb::new("chapter_items");
    let conn = xbible_engine::db::open(db.path()).unwrap();

    let insert = "INSERT INTO chapter_items
        (translation_id, book_id, chapter, position, item_type,
         verse_number, text, poem, words_of_jesus, footnote_ids, raw_json)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)";

    conn.execute(
        insert,
        rusqlite::params![
            "BSB",
            "GEN",
            1,
            0,
            "heading",
            None::<i32>,
            "The Creation",
            0,
            0,
            None::<String>,
            "{}"
        ],
    )
    .unwrap();

    conn.execute(
        insert,
        rusqlite::params![
            "BSB",
            "GEN",
            1,
            1,
            "verse",
            Some(1),
            "In the beginning...",
            0,
            1,
            "fn1,fn2",
            "{}"
        ],
    )
    .unwrap();

    let mut stmt = conn
        .prepare(
            "SELECT item_type, verse_number, text, poem, words_of_jesus, footnote_ids
             FROM chapter_items
             WHERE translation_id = 'BSB' AND book_id = 'GEN' AND chapter = 1
             ORDER BY position",
        )
        .unwrap();

    let rows: Vec<(
        String,
        Option<i32>,
        Option<String>,
        i32,
        i32,
        Option<String>,
    )> = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })
        .unwrap()
        .filter_map(|x| x.ok())
        .collect();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, "heading");
    assert_eq!(rows[0].2.as_deref(), Some("The Creation"));
    assert_eq!(rows[1].0, "verse");
    assert_eq!(rows[1].1, Some(1));
    assert_eq!(rows[1].4, 1, "words_of_jesus should be 1");
    assert_eq!(rows[1].5.as_deref(), Some("fn1,fn2"));
}

// ── FTS5 index sync ────────────────────────────────────────────

#[test]
fn fts_index_populates_on_insert_and_clears_on_delete() {
    let db = TempDb::new("fts_sync");
    let conn = xbible_engine::db::open(db.path()).expect("open failed");

    // Seed the catalog row so persist_book's UPDATE succeeds.
    conn.execute(
        "INSERT INTO translations
            (id, name, english_name, short_name, language, text_direction,
             number_of_books, total_number_of_chapters, total_number_of_verses,
             installed, has_strongs)
         VALUES ('TST','Test','Test','TST','eng','ltr',1,2,3,0,0)",
        [],
    )
    .unwrap();

    // Build a FreeBibleCompleteBook with two chapters. This is the
    // exact type persist_book consumes, so the test exercises the
    // real install write path, not a raw INSERT.
    use xbible_engine::engines::freebible_engine::client::{
        FreeBibleCompleteBook, FreeBibleCompleteChapter, FreeBibleVerse,
    };

    let book = FreeBibleCompleteBook {
        id: "GEN".into(),
        name: "Genesis".into(),
        number_of_chapters: 2,
        common_name: Some("Genesis".into()),
        title: Some("Genesis".into()),
        book_order: Some(1),
        audio_links_json: None,
        raw_json: "{}".into(),
        chapters: vec![
            FreeBibleCompleteChapter {
                number: 1,
                verses: vec![FreeBibleVerse {
                    number: 1,
                    text: "In the beginning God created the heavens and the earth.".into(),
                }],
                items: vec![],
                footnotes: vec![],
                audio_timings: vec![],
                raw_json: "{}".into(),
            },
            FreeBibleCompleteChapter {
                number: 2,
                verses: vec![FreeBibleVerse {
                    number: 1,
                    text: "Thus the heavens and the earth were completed.".into(),
                }],
                items: vec![],
                footnotes: vec![],
                audio_timings: vec![],
                raw_json: "{}".into(),
            },
        ],
    };

    xbible_engine::engines::freebible_engine::install::persist_book(&conn, "TST", &book)
        .expect("persist_book");

    // FTS index should now contain both verses. Query it directly
    // so the assertion is not passing through search.rs yet.
    let fts_count: i32 = conn
        .query_row("SELECT COUNT(*) FROM verses_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        fts_count, 2,
        "verses_fts should have 2 rows after persist_book"
    );

    // Now run a real search through the search module.
    let result = xbible_engine::engines::freebible_engine::search::search(
        &conn,
        "beginning",
        Some("TST"),
        10,
        |_| {},
    )
    .expect("search");
    assert_eq!(result.total_hits, 1, "should find exactly one 'beginning'");
    assert_eq!(result.hits[0].chapter, 1);
    assert_eq!(result.hits[0].verse, 1);
    assert!(result.hits[0].text.contains("In the beginning"));

    // Delete one verse; the trigger must remove it from the index.
    conn.execute(
        "DELETE FROM verses WHERE translation_id='TST' AND book_id='GEN' AND chapter=2",
        [],
    )
    .unwrap();

    let fts_count_after: i32 = conn
        .query_row("SELECT COUNT(*) FROM verses_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        fts_count_after, 1,
        "verses_fts should have 1 row after deleting one verse"
    );

    // And the deleted verse's unique word ("completed") must be gone.
    let result_after = xbible_engine::engines::freebible_engine::search::search(
        &conn,
        "completed",
        Some("TST"),
        10,
        |_| {},
    )
    .expect("search after delete");
    assert_eq!(
        result_after.total_hits, 0,
        "deleted verse should not be searchable"
    );
}

#[test]
fn fts_index_handles_update() {
    let db = TempDb::new("fts_update");
    let conn = xbible_engine::db::open(db.path()).expect("open failed");

    conn.execute(
        "INSERT INTO translations
            (id, name, english_name, short_name, language, text_direction,
             number_of_books, total_number_of_chapters, total_number_of_verses,
             installed, has_strongs)
         VALUES ('TST','Test','Test','TST','eng','ltr',1,1,1,0,0)",
        [],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO books (translation_id, id, name, number_of_chapters)
         VALUES ('TST','GEN','Genesis',1)",
        [],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO verses (translation_id, book_id, chapter, verse, text)
         VALUES ('TST','GEN',1,1,'original text')",
        [],
    )
    .unwrap();

    // UPDATE must remove the old token and add the new one.
    // `verses_fts_au` does delete + reinsert.
    conn.execute(
        "UPDATE verses SET text='replacement text' WHERE translation_id='TST' AND book_id='GEN' AND chapter=1 AND verse=1",
        [],
    )
    .unwrap();

    let old = xbible_engine::engines::freebible_engine::search::search(
        &conn,
        "original",
        Some("TST"),
        10,
        |_| {},
    )
    .unwrap();
    assert_eq!(old.total_hits, 0, "old token should be gone after update");

    let new = xbible_engine::engines::freebible_engine::search::search(
        &conn,
        "replacement",
        Some("TST"),
        10,
        |_| {},
    )
    .unwrap();
    assert_eq!(
        new.total_hits, 1,
        "new token should be indexed after update"
    );
}
