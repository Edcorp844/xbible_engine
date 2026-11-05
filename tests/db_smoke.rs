fn temp_db_path(name: &str) -> String {
    let mut p = std::env::temp_dir();
    p.push(format!("xbible_test_{}_{}.db", name, std::process::id()));
    let _ = std::fs::remove_file(&p);
    let _ = std::fs::remove_file(format!("{}-wal", p.to_string_lossy()));
    let _ = std::fs::remove_file(format!("{}-shm", p.to_string_lossy()));
    p.to_string_lossy().into_owned()
}

fn cleanup(path: &str) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path));
    let _ = std::fs::remove_file(format!("{}-shm", path));
}

#[test]
fn db_creates_tables_and_sets_version() {
    let path = temp_db_path("schema");
    let conn = xbible_engine::db::open(&path).expect("open failed");

    let version: i32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert!(
        version >= 1,
        "expected at least 1 migration, got {}",
        version
    );

    // Every table the migrations create must exist.
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

    // Columns added by v4 via ALTER TABLE must be present.
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

    drop(conn);

    // Idempotent reopen — version must not change, migrations must not re-run.
    let conn2 = xbible_engine::db::open(&path).expect("reopen failed");
    let v2: i32 = conn2
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v2, version, "reopen changed the version");
    drop(conn2);

    cleanup(&path);
}

#[test]
fn wal_mode_is_enabled() {
    let path = temp_db_path("wal");
    let conn = xbible_engine::db::open(&path).unwrap();

    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal");

    drop(conn);
    cleanup(&path);
}

#[test]
fn chapter_items_survive_insert_and_query() {
    let path = temp_db_path("chapter_items");
    let conn = xbible_engine::db::open(&path).unwrap();

    // Minimal row: a heading followed by a verse, in order.
    conn.execute(
        "INSERT INTO chapter_items
            (translation_id, book_id, chapter, position, item_type,
             verse_number, text, poem, words_of_jesus, footnote_ids, raw_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
        "INSERT INTO chapter_items
            (translation_id, book_id, chapter, position, item_type,
             verse_number, text, poem, words_of_jesus, footnote_ids, raw_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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

    drop(stmt);
    drop(conn);
    cleanup(&path);
}
