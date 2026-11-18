//! Integration tests for the shared SQLite layer, generic models, and search.
//!
//! These live in `tests/` (a separate crate), so they reach the
//! engine through `xbible_engine::` as an external consumer would.

use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, params};
use xbible_engine::features::reader::search::{SearchFilter, SearchMode, search};
use xbible_engine::sources::models::{
    CanonicalSection, GenericAudioSource, GenericBook, GenericChapter, GenericChapterItem,
    GenericFootnote, GenericTextSpan, GenericVerse, LexicalInfo, SpanFormatting, SpanKind,
    TextDirection,
};

// ── Temp file guard ─────────────────────────────────────────────

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
        let _ = std::fs::remove_file(format!("{path}-journal"));
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
        params![table, column],
        |r| r.get::<_, i32>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

fn user_version(conn: &Connection) -> i32 {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0)
}

fn section_db_key(section: &CanonicalSection) -> &'static str {
    match section {
        CanonicalSection::OldTestament => "ot",
        CanonicalSection::NewTestament => "nt",
        CanonicalSection::Deuterocanon => "deutero",
        CanonicalSection::OrthodoxExtra => "orthodox_extra",
        CanonicalSection::EthiopianExtra => "ethiopian_extra",
        CanonicalSection::Unknown => "unknown",
        CanonicalSection::Custom(_) => "custom",
    }
}

fn seed_translation(conn: &Connection, id: &str, source_id: &str) {
    conn.execute(
        "INSERT INTO translations
            (id, name, english_name, short_name, language, language_code,
             text_direction, number_of_books, total_number_of_chapters,
             total_number_of_verses, installed, has_strongs, source_id)
         VALUES (?1, 'Test Bible', 'Test Bible', ?1, 'eng', 'en',
                 'ltr', 2, 2, 4, 1, 1, ?2)",
        params![id, source_id],
    )
    .unwrap();
}

fn persist_generic_book_and_chapters(
    conn: &Connection,
    book: &GenericBook,
    chapters: &[GenericChapter],
) {
    conn.execute(
        "INSERT OR REPLACE INTO books (translation_id, id, name, number_of_chapters)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            book.translation_id,
            book.id,
            book.name,
            book.number_of_chapters
        ],
    )
    .unwrap();

    conn.execute(
        "INSERT OR REPLACE INTO book_metadata
            (translation_id, book_id, common_name, title, book_order,
             audio_links_json, raw_json, section)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            book.translation_id,
            book.id,
            book.common_name,
            book.title,
            book.book_order,
            book.audio_links_json,
            book.raw_json,
            section_db_key(&book.section),
        ],
    )
    .unwrap();

    for ch in chapters {
        conn.execute(
            "INSERT OR REPLACE INTO chapter_raw
                (translation_id, book_id, chapter, raw_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                ch.translation_id,
                ch.book_id,
                ch.chapter_number,
                ch.raw_json
            ],
        )
        .unwrap();

        for item in &ch.items {
            let footnotes_csv = if item.footnote_ids.is_empty() {
                None
            } else {
                Some(item.footnote_ids.join(","))
            };
            conn.execute(
                "INSERT OR REPLACE INTO chapter_items
                    (translation_id, book_id, chapter, position, item_type,
                     verse_number, text, poem, words_of_jesus, footnote_ids, raw_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    ch.translation_id,
                    ch.book_id,
                    ch.chapter_number,
                    item.position,
                    item.item_type,
                    item.verse_number,
                    item.text,
                    i32::from(item.is_poem),
                    i32::from(item.is_words_of_jesus),
                    footnotes_csv,
                    item.raw_json,
                ],
            )
            .unwrap();
        }

        for v in &ch.verses {
            conn.execute(
                "INSERT OR REPLACE INTO verses
                    (translation_id, book_id, chapter, verse, text)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![ch.translation_id, v.book_id, v.chapter, v.verse, v.text],
            )
            .unwrap();
        }

        for note in &ch.footnotes {
            conn.execute(
                "INSERT OR REPLACE INTO footnotes
                    (translation_id, book_id, chapter, note_id, caller, text, reference, raw_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    ch.translation_id,
                    ch.book_id,
                    ch.chapter_number,
                    note.note_id,
                    note.caller,
                    note.text,
                    note.reference,
                    note.raw_json,
                ],
            )
            .unwrap();
        }
    }
}

// ── Schema ──────────────────────────────────────────────────────

#[test]
fn db_creates_tables_and_sets_version() {
    let db = TempDb::new("schema");
    let conn = xbible_engine::db::open(db.path()).expect("open failed");

    let v = user_version(&conn);
    assert!(v >= 10, "expected migrated schema version >= 10, got {v}");

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
        "people",
        "events",
        "places",
        "people_groups",
        "entity_refs",
        "entity_verse_refs",
        "entity_install_state",
        "commentary_books",
        "commentary_chapters",
        "verses_fts",
    ] {
        assert!(table_exists(&conn, table), "missing table {table}");
    }

    for (table, column) in [
        ("translations", "license_url"),
        ("translations", "license_notes"),
        ("translations", "raw_json"),
        ("translations", "selected_canon"),
        ("translations", "source_id"),
        ("translations", "language_code"),
        ("translations", "book_count"),
        ("translations", "chapter_count"),
        ("translations", "verse_count"),
        ("translations", "has_text"),
        ("translations", "has_audio"),
        ("translations", "audio_sources_json"),
        ("commentaries", "license_notes"),
        ("commentaries", "raw_json"),
        ("datasets", "license_notes"),
        ("datasets", "raw_json"),
        ("book_metadata", "section"),
    ] {
        assert!(
            column_exists(&conn, table, column),
            "missing column {table}.{column}"
        );
    }

    drop(conn);

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

// ── Generic models & content round-trip ─────────────────────────

#[test]
fn generic_chapter_items_and_spans_round_trip() {
    let db = TempDb::new("generic_chapter_items");
    let conn = xbible_engine::db::open(db.path()).unwrap();
    seed_translation(&conn, "BSB", "helloao");

    let book = GenericBook {
        translation_id: "BSB".into(),
        source_id: "helloao".into(),
        id: "GEN".into(),
        name: "Genesis".into(),
        common_name: Some("Genesis".into()),
        title: Some("The First Book of Moses, Called Genesis".into()),
        book_order: Some(1),
        section: CanonicalSection::OldTestament,
        number_of_chapters: 1,
        audio_sources: vec![GenericAudioSource {
            source_ref_id: "main".into(),
            url: Some("https://example.invalid/gen/1.mp3".into()),
            format: Some("mp3".into()),
            is_dramatized: Some(false),
        }],
        audio_links_json: Some(r#"{"main":"https://example.invalid/gen/1.mp3"}"#.into()),
        raw_json: Some(r#"{"id":"GEN"}"#.into()),
    };

    let chapter = GenericChapter {
        translation_id: "BSB".into(),
        source_id: "helloao".into(),
        book_id: "GEN".into(),
        chapter_number: 1,
        items: vec![
            GenericChapterItem {
                position: 0,
                item_type: "heading".into(),
                verse_number: None,
                text: Some("The Creation".into()),
                is_poem: false,
                is_words_of_jesus: false,
                footnote_ids: Vec::new(),
                spans: vec![GenericTextSpan {
                    position: 0,
                    kind: SpanKind::Text,
                    text: "The Creation".into(),
                    formatting: SpanFormatting::default(),
                    lexical: None,
                    footnote_ids: Vec::new(),
                    cross_ref_ids: Vec::new(),
                    language_code: Some("en".into()),
                    raw_json: None,
                }],
                raw_json: Some(r#"{"type":"heading"}"#.into()),
            },
            GenericChapterItem {
                position: 1,
                item_type: "verse".into(),
                verse_number: Some(1),
                text: Some("In the beginning God created".into()),
                is_poem: false,
                is_words_of_jesus: true,
                footnote_ids: vec!["fn1".into(), "fn2".into()],
                spans: vec![GenericTextSpan {
                    position: 0,
                    kind: SpanKind::Word,
                    text: "God".into(),
                    formatting: SpanFormatting {
                        is_words_of_jesus: true,
                        ..SpanFormatting::default()
                    },
                    lexical: Some(LexicalInfo {
                        strongs: Some("H430".into()),
                        lemma: Some("אֱלֹהִים".into()),
                        transliteration: Some("elohim".into()),
                        part_of_speech: Some("noun".into()),
                        morphology: None,
                        gloss: Some("God".into()),
                    }),
                    footnote_ids: vec!["fn1".into()],
                    cross_ref_ids: Vec::new(),
                    language_code: Some("hbo".into()),
                    raw_json: None,
                }],
                raw_json: Some(r#"{"type":"verse","number":1}"#.into()),
            },
        ],
        verses: vec![GenericVerse {
            book_id: "GEN".into(),
            chapter: 1,
            verse: 1,
            text: "In the beginning God created".into(),
        }],
        footnotes: vec![GenericFootnote {
            note_id: "fn1".into(),
            caller: Some("a".into()),
            text: Some("Hebrew Elohim".into()),
            reference: None,
            raw_json: Some(r#"{"noteId":"fn1"}"#.into()),
        }],
        audio_sources: Vec::new(),
        audio_timings: Vec::new(),
        raw_json: r#"{"chapter":1}"#.into(),
    };

    assert_eq!(TextDirection::default(), TextDirection::LeftToRight);
    persist_generic_book_and_chapters(&conn, &book, &[chapter]);

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
    seed_translation(&conn, "TST", "helloao");

    let book = GenericBook {
        translation_id: "TST".into(),
        source_id: "helloao".into(),
        id: "GEN".into(),
        name: "Genesis".into(),
        common_name: Some("Genesis".into()),
        title: Some("Genesis".into()),
        book_order: Some(1),
        section: CanonicalSection::OldTestament,
        number_of_chapters: 2,
        audio_sources: Vec::new(),
        audio_links_json: None,
        raw_json: Some("{}".into()),
    };

    let chapters = vec![
        GenericChapter {
            translation_id: "TST".into(),
            source_id: "helloao".into(),
            book_id: "GEN".into(),
            chapter_number: 1,
            items: vec![],
            verses: vec![GenericVerse {
                book_id: "GEN".into(),
                chapter: 1,
                verse: 1,
                text: "In the beginning God created the heavens and the earth.".into(),
            }],
            footnotes: vec![],
            audio_sources: Vec::new(),
            audio_timings: Vec::new(),
            raw_json: "{}".into(),
        },
        GenericChapter {
            translation_id: "TST".into(),
            source_id: "helloao".into(),
            book_id: "GEN".into(),
            chapter_number: 2,
            items: vec![],
            verses: vec![GenericVerse {
                book_id: "GEN".into(),
                chapter: 2,
                verse: 1,
                text: "Thus the heavens and the earth were completed.".into(),
            }],
            footnotes: vec![],
            audio_sources: Vec::new(),
            audio_timings: Vec::new(),
            raw_json: "{}".into(),
        },
    ];

    persist_generic_book_and_chapters(&conn, &book, &chapters);

    let fts_count: i32 = conn
        .query_row("SELECT COUNT(*) FROM verses_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        fts_count, 2,
        "verses_fts should have 2 rows after persisting chapters"
    );

    let result = search(
        &conn,
        "beginning",
        Some("TST"),
        10,
        SearchFilter::default(),
        |_| {},
    )
    .expect("search");
    assert_eq!(result.total_hits, 1, "should find exactly one 'beginning'");
    assert_eq!(result.hits[0].chapter, 1);
    assert_eq!(result.hits[0].verse, 1);
    assert!(result.hits[0].text.contains("In the beginning"));

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

    let result_after = search(
        &conn,
        "completed",
        Some("TST"),
        10,
        SearchFilter::default(),
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
    seed_translation(&conn, "TST", "helloao");

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

    conn.execute(
        "UPDATE verses SET text='replacement text' WHERE translation_id='TST' AND book_id='GEN' AND chapter=1 AND verse=1",
        [],
    )
    .unwrap();

    let old = search(
        &conn,
        "original",
        Some("TST"),
        10,
        SearchFilter::default(),
        |_| {},
    )
    .unwrap();
    assert_eq!(old.total_hits, 0, "old token should be gone after update");

    let new = search(
        &conn,
        "replacement",
        Some("TST"),
        10,
        SearchFilter::default(),
        |_| {},
    )
    .unwrap();
    assert_eq!(
        new.total_hits, 1,
        "new token should be indexed after update"
    );
}

// ── Search filter ──────────────────────────────────────────────

#[test]
fn search_filter_restricts_to_book() {
    let db = TempDb::new("filter_book");
    let conn = xbible_engine::db::open(db.path()).expect("open failed");
    seed_translation(&conn, "TST", "helloao");

    conn.execute(
        "INSERT INTO books (translation_id, id, name, number_of_chapters)
         VALUES ('TST','GEN','Genesis',1), ('TST','PSA','Psalms',1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO book_metadata (translation_id, book_id, book_order, section)
         VALUES ('TST','GEN',1,'ot'), ('TST','PSA',19,'ot')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO verses (translation_id, book_id, chapter, verse, text)
         VALUES ('TST','GEN',1,1,'In the beginning God created the heavens.'),
                ('TST','PSA',1,1,'The LORD is my shepherd; God is with me.')",
        [],
    )
    .unwrap();

    let no_filter = search(
        &conn,
        "God",
        Some("TST"),
        10,
        SearchFilter::default(),
        |_| {},
    )
    .unwrap();
    assert_eq!(no_filter.total_hits, 2);

    let psa_only = search(
        &conn,
        "God",
        Some("TST"),
        10,
        SearchFilter {
            book_ids: vec!["PSA".into()],
            ..SearchFilter::default()
        },
        |_| {},
    )
    .unwrap();
    assert_eq!(psa_only.total_hits, 1);
    assert_eq!(psa_only.hits[0].book_id, "PSA");

    let both = search(
        &conn,
        "God",
        Some("TST"),
        10,
        SearchFilter {
            book_ids: vec!["GEN".into(), "PSA".into()],
            ..SearchFilter::default()
        },
        |_| {},
    )
    .unwrap();
    assert_eq!(both.total_hits, 2);

    let jhn_only = search(
        &conn,
        "God",
        Some("TST"),
        10,
        SearchFilter {
            book_ids: vec!["JHN".into()],
            ..SearchFilter::default()
        },
        |_| {},
    )
    .unwrap();
    assert_eq!(jhn_only.total_hits, 0);
}

// ── Search mode ────────────────────────────────────────────────

#[test]
fn search_mode_phrase_requires_adjacent_words() {
    let db = TempDb::new("mode_phrase");
    let conn = xbible_engine::db::open(db.path()).expect("open failed");
    seed_translation(&conn, "TST", "helloao");

    conn.execute(
        "INSERT INTO books (translation_id, id, name, number_of_chapters)
         VALUES ('TST','JHN','John',1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO verses (translation_id, book_id, chapter, verse, text)
         VALUES ('TST','JHN',1,1,'love God'),
                ('TST','JHN',1,2,'God is love'),
                ('TST','JHN',1,3,'God''s love')",
        [],
    )
    .unwrap();

    let multi = search(
        &conn,
        "love God",
        Some("TST"),
        10,
        SearchFilter {
            mode: SearchMode::MultiWord,
            ..SearchFilter::default()
        },
        |_| {},
    )
    .unwrap();
    assert_eq!(multi.total_hits, 3, "multi-word should match all three");

    let phrase = search(
        &conn,
        "love God",
        Some("TST"),
        10,
        SearchFilter {
            mode: SearchMode::Phrase,
            ..SearchFilter::default()
        },
        |_| {},
    )
    .unwrap();
    assert_eq!(phrase.total_hits, 1, "phrase should match only verse 1");
    assert_eq!(phrase.hits[0].verse, 1);
}

#[test]
fn search_mode_regex_matches_pattern() {
    let db = TempDb::new("mode_regex");
    let conn = xbible_engine::db::open(db.path()).expect("open failed");
    seed_translation(&conn, "TST", "helloao");

    conn.execute(
        "INSERT INTO books (translation_id, id, name, number_of_chapters)
         VALUES ('TST','PSA','Psalms',1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO verses (translation_id, book_id, chapter, verse, text)
         VALUES ('TST','PSA',1,1,'The LORD is my shepherd'),
                ('TST','PSA',1,2,'I shall not want')",
        [],
    )
    .unwrap();

    let result = search(
        &conn,
        r"LORD\b",
        Some("TST"),
        10,
        SearchFilter {
            mode: SearchMode::Regex,
            ..SearchFilter::default()
        },
        |_| {},
    )
    .unwrap();
    assert_eq!(result.total_hits, 1, "only verse 1 contains LORD");
    assert_eq!(result.hits[0].verse, 1);
    assert_eq!(result.hits[0].book_id, "PSA");
    assert!(!result.hits[0].highlights.is_empty());
}
