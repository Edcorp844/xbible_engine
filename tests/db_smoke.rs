//! Integration tests for the shared SQLite layer.
//!
//! These live inside the crate (not in `tests/`) because `db::open`
//! is `pub(crate)` — the raw opener is an internal escape hatch, and
//! the public entry point is `Db::<Closed>::open`. Testing the real
//! file-backed path, WAL, and migrations requires reaching the
//! internal opener, so these are unit tests by Rust's definition.

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
            "BSB", "GEN", 1, 0, "heading",
            None::<i32>, "The Creation", 0, 0, None::<String>, "{}"
        ],
    )
    .unwrap();

    conn.execute(
        insert,
        rusqlite::params![
            "BSB", "GEN", 1, 1, "verse",
            Some(1), "In the beginning...", 0, 1, "fn1,fn2", "{}"
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