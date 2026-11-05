use std::thread;
use std::time::Duration;

use xbible_engine::engines::freebible_engine::store_api::FreeBibleStore;
use xbible_engine::engines::freebible_engine::utils::FreeBibleTaskState;

fn main() {

    xbible_engine::init_logging();

    let path = std::env::temp_dir().join("xbible_smoke.db");
    cleanup(&path);

    let store = FreeBibleStore::new();
    store
        .set_db_path(path.to_string_lossy().into_owned())
        .expect("db open failed");

    // ── 1. Catalog refresh ───────────────────────────────────────
    println!("Refreshing catalog...");
    store.refresh().expect("refresh failed");
    println!("Languages: {}", store.languages().len());

    let translations_before = count_rows(&path, "translations");
    println!("Rows in translations table: {translations_before}");

    // ── 2. Install BSB ───────────────────────────────────────────
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

    // ── 3. Verify install landed in DB ───────────────────────────
    let verses = count_verses(&path, translation_id);
    let books = count_books(&path, translation_id);
    let items = count_rows_where(&path, "chapter_items", "translation_id", translation_id);
    let raw_chapters = count_rows_where(&path, "chapter_raw", "translation_id", translation_id);
    let notes = count_rows_where(&path, "footnotes", "translation_id", translation_id);
    let installed = store.is_installed(translation_id.into());

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
        raw_chapters, verses.max(1) * 0 + 1189,
        "expected 1189 chapter_raw rows (one per chapter), got {raw_chapters}"
    );

    // ── 4. Read path — books + chapter + items ───────────────────
    println!("\n=== READ PATH ===");

    let book_list = store.get_books(translation_id.into());
    println!("get_books → {}", book_list.len());
    if let Some(first) = book_list.first() {
        println!(
            "  first: {} (id={}, order={:?}, chapters={})",
            first.name, first.id, first.book_order, first.number_of_chapters
        );
    }
    assert_eq!(book_list.len(), 66);

    let gen1 = store.get_chapter(translation_id.into(), "GEN".into(), 1);
    println!("get_chapter GEN 1 → {} verses", gen1.len());
    assert!(!gen1.is_empty(), "GEN 1 returned no verses");

    let gen1_items = store.get_chapter_items(translation_id.into(), "GEN".into(), 1);
    println!("get_chapter_items GEN 1 → {} items", gen1_items.len());
    for item in gen1_items.iter().take(5) {
        let preview = item
            .text
            .as_deref()
            .map(|s| &s[..s.len().min(60)])
            .unwrap_or("<none>");
        println!(
            "  pos={:>3} {:16} verse={:?} text={}",
            item.position, item.item_type, item.verse_number, preview
        );
    }
    assert!(!gen1_items.is_empty(), "GEN 1 returned no chapter_items");

    let john_3_16 = store.get_verse(translation_id.into(), "JHN".into(), 3, 16);
    println!(
        "\nJohn 3:16: {}",
        john_3_16
            .as_deref()
            .map(|s| &s[..s.len().min(80)])
            .unwrap_or("<not found>")
    );
    assert!(john_3_16.is_some(), "John 3:16 should be readable");

    let ps23_notes = store.get_footnotes(translation_id.into(), "PSA".into(), 23);
    println!("PSA 23 footnotes: {}", ps23_notes.len());

    // ── 5. Uninstall ─────────────────────────────────────────────
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

    let verses_after = count_verses(&path, translation_id);
    let items_after = count_rows_where(&path, "chapter_items", "translation_id", translation_id);
    let installed_after = store.is_installed(translation_id.into());

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

fn count_verses(path: &std::path::Path, translation_id: &str) -> i32 {
    count_rows_where(path, "verses", "translation_id", translation_id)
}

fn count_books(path: &std::path::Path, translation_id: &str) -> i32 {
    count_rows_where(path, "books", "translation_id", translation_id)
}