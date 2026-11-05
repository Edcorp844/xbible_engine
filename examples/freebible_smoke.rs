use std::thread;
use std::time::Duration;

use xbible_engine::engines::freebible_engine::utils::FreeBibleTaskState;
use xbible_engine::engines::xbible_engine::engine::XBibleEngine;

fn main() {
    xbible_engine::init_logging();

    let path = std::env::temp_dir().join("xbible_smoke.db");
    cleanup(&path);

    // ── 0. Open the top-level engine ────────────────────────────
    let engine = XBibleEngine::new();

    // Reach through XBibleEngine to its FreeBible sub-engine,
    // then hand it the DB path via the FreeBible engine's own
    // set_db_path (which opens the connection and shares it with
    // both store and reader).
    engine
        .freebible_engine
        .set_db_path(path.to_string_lossy().into_owned())
        .expect("freebible db open failed");

    // Sub-handles. These are Arc<FreeBibleStore> / Arc<FreeBibleReader>
    // and share the same connection.
    let store = engine.freebible_engine.store();
    let reader = engine.freebible_engine.reader();

    // ── 1. Catalog refresh ──────────────────────────────────────
    println!("Refreshing catalog...");
    store.refresh().expect("refresh failed");
    println!("Languages: {}", store.languages().len());

    let translations_before = count_rows(&path, "translations");
    println!("Rows in translations table: {translations_before}");

    // ── 2. Install BSB ──────────────────────────────────────────
    let translation_id = "BSB";
    println!("\nInstalling {translation_id}...");
    let task_id = store
        .install_translation(translation_id.into())
        .expect("install start failed");

    let mut last_progress_pct = -1.0f64;
    loop {
        thread::sleep(Duration::from_millis(500));

        let Some(status) = store.poll_task(task_id.clone()) else {
            println!("  task disappeared");
            break;
        };

        let p = &status.progress;
        let pct = p.fraction * 100.0;
        if pct - last_progress_pct >= 5.0 || pct >= 100.0 {
            last_progress_pct = pct;
            println!(
                "  [{:>5.1}%] {} (ch {}/{}, verses {})",
                pct, p.message, p.chapters_done, p.chapters_total, p.verses_done,
            );
        }

        match status.state {
            FreeBibleTaskState::Running | FreeBibleTaskState::Queued => continue,
            FreeBibleTaskState::Completed => {
                println!("  ✓ install complete");
                break;
            }
            FreeBibleTaskState::Failed { error } => {
                println!("  ✗ install failed: {error}");
                return;
            }
            FreeBibleTaskState::Cancelled => {
                println!("  ✗ install cancelled");
                return;
            }
        }
    }

    // ── 3. Verify install landed in DB ──────────────────────────
    let verses = count_rows_where(&path, "verses", "translation_id", translation_id);
    let books = count_rows_where(&path, "books", "translation_id", translation_id);
    let items = count_rows_where(&path, "chapter_items", "translation_id", translation_id);
    let raw_chapters = count_rows_where(&path, "chapter_raw", "translation_id", translation_id);
    let notes = count_rows_where(&path, "footnotes", "translation_id", translation_id);
    let installed = reader.is_installed(translation_id.into());

    println!("\nAfter install:");
    println!("  books:         {books}");
    println!("  verses:        {verses}");
    println!("  chapter_items: {items}");
    println!("  chapter_raw:   {raw_chapters}");
    println!("  footnotes:     {notes}");
    println!("  installed:     {installed}");

    assert!(installed, "is_installed returned false after successful install");
    assert!(verses > 30_000, "expected >30k verses for BSB, got {verses}");
    assert_eq!(books, 66, "expected 66 books for BSB, got {books}");
    assert!(items > 20_000, "expected >20k chapter items, got {items}");
    assert_eq!(
        raw_chapters, 1189,
        "expected 1189 chapter_raw rows (one per chapter), got {raw_chapters}"
    );

    // ── 4. Read path — via the reader ───────────────────────────
    println!("\n=== READ PATH ===");

    let installed_list = reader.get_installed_translations();
    println!("get_installed_translations → {}", installed_list.len());
    assert!(
        installed_list.iter().any(|t| t.id == translation_id),
        "installed list does not contain {translation_id}"
    );

    let book_list = reader.get_books(translation_id.into());
    println!("get_books → {}", book_list.len());
    if let Some(first) = book_list.first() {
        println!(
            "  first: {} (id={}, order={:?}, chapters={})",
            first.name, first.id, first.book_order, first.number_of_chapters
        );
    }
    assert_eq!(book_list.len(), 66);

    let gen1 = reader
        .read_chapter(translation_id.into(), "GEN".into(), 1)
        .expect("GEN 1 not found");
    println!("\nread_chapter GEN 1:");
    println!("  book_name:      {}", gen1.book_name);
    println!("  chapter_number: {}", gen1.chapter_number);
    println!("  blocks:         {}", gen1.blocks.len());
    println!("  footnotes:      {}", gen1.footnotes.len());
    println!("  prev: {:?}  next: {:?}", gen1.prev_chapter, gen1.next_chapter);

    for block in gen1.blocks.iter().take(5) {
        let preview = block.text.chars().take(60).collect::<String>();
        println!(
            "  [{:16}] verse={:?} poem={} red={} text={}",
            block.item_type,
            block.verse_number,
            block.is_poem,
            block.is_words_of_jesus,
            preview,
        );
    }

    assert!(
        gen1.blocks.iter().any(|b| b.item_type == "verse"),
        "GEN 1 has no verse blocks"
    );

    let john_3_16 = reader.get_verse(translation_id.into(), "JHN".into(), 3, 16);
    println!(
        "\nJohn 3:16: {}",
        john_3_16
            .as_deref()
            .map(|s| s.chars().take(80).collect::<String>())
            .unwrap_or_else(|| "<not found>".into())
    );
    assert!(john_3_16.is_some(), "John 3:16 should be readable");

    let mat5 = reader
        .read_chapter(translation_id.into(), "MAT".into(), 5)
        .expect("MAT 5 not found");
    let has_red_letters = mat5
        .blocks
        .iter()
        .flat_map(|b| b.verses.iter())
        .flat_map(|v| v.words.iter())
        .any(|w| w.is_words_of_jesus);
    println!("MAT 5 has red-letter words: {has_red_letters}");

    let ps23 = reader
        .read_chapter(translation_id.into(), "PSA".into(), 23)
        .expect("PSA 23 not found");
    println!("PSA 23 footnotes: {}", ps23.footnotes.len());

    // ── 5. Uninstall ────────────────────────────────────────────
    println!("\nUninstalling {translation_id}...");
    let task_id = store
        .uninstall_translation(translation_id.into())
        .expect("uninstall start failed");

    loop {
        thread::sleep(Duration::from_millis(200));
        let Some(status) = store.poll_task(task_id.clone()) else {
            break;
        };
        match status.state {
            FreeBibleTaskState::Running | FreeBibleTaskState::Queued => continue,
            FreeBibleTaskState::Completed => {
                println!("  ✓ uninstall complete");
                break;
            }
            FreeBibleTaskState::Failed { error } => {
                println!("  ✗ uninstall failed: {error}");
                return;
            }
            FreeBibleTaskState::Cancelled => {
                println!("  ✗ uninstall cancelled");
                return;
            }
        }
    }

    let verses_after = count_rows_where(&path, "verses", "translation_id", translation_id);
    let items_after = count_rows_where(&path, "chapter_items", "translation_id", translation_id);
    let installed_after = reader.is_installed(translation_id.into());

    println!("\nAfter uninstall:");
    println!("  verses:        {verses_after}");
    println!("  chapter_items: {items_after}");
    println!("  installed:     {installed_after}");

    assert_eq!(verses_after, 0, "verses remain after uninstall");
    assert_eq!(items_after, 0, "chapter_items remain after uninstall");
    assert!(!installed_after, "installed flag still true after uninstall");

    cleanup(&path);
    println!("\nDone.");
}

// ── Helpers ──────────────────────────────────────────────────────

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path.to_string_lossy()));
    let _ = std::fs::remove_file(format!("{}-shm", path.to_string_lossy()));
}

fn count_rows(path: &std::path::Path, table: &str) -> i32 {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn count_rows_where(
    path: &std::path::Path,
    table: &str,
    column: &str,
    value: &str,
) -> i32 {
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.query_row(
        &format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1"),
        rusqlite::params![value],
        |r| r.get(0),
    )
    .unwrap()
}