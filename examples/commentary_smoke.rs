//! Install a commentary, read a chapter from it, uninstall it.
//!
//! Commentary endpoints use the `/c/` prefix, not the translation prefix:
//!   /api/c/{commentary}/books.json
//!   /api/c/{commentary}/{book}/{chapter}.json
//!
//! Commentaries have no `complete.json`, so the install fetches the
//! book list once and then per-chapter.

use std::thread;
use std::time::Duration;

use xbible_engine::engines::freebible_engine::utils::FreeBibleTaskState;
use xbible_engine::XBibleEngine;

const COMMENTARY: &str = "adam-clarke";

fn main() {
    let db_path = std::env::temp_dir().join(format!(
        "commentary_smoke_{}.db",
        std::process::id()
    ));
    cleanup(&db_path);

    let engine = XBibleEngine::new();
    engine
        .freebible()
        .set_db_path(db_path.to_string_lossy().into_owned())
        .expect("engine open failed");

    let store = engine.freebible().store();
    let reader = engine.freebible().reader();

    // ── 1. Refresh the catalog so `commentaries` has the row ────
    println!("Refreshing catalog…");
    store.refresh().expect("refresh failed");

    println!("Catalog refreshed. Looking for {COMMENTARY}…");

    // ── 2. Install the commentary ───────────────────────────────
    println!("\nInstalling commentary {COMMENTARY}…");
    let task_id = store
        .install_commentary(COMMENTARY.into())
        .expect("commentary install start failed");
    wait_for_task(&store, &task_id);

    // ── 3. Verify the install landed ────────────────────────────
    let installed = reader
        .is_commentary_installed(COMMENTARY.into())
        .expect("is_commentary_installed failed");
    println!("\nAfter install:");
    println!("  installed: {}", installed);
    assert!(installed, "{COMMENTARY} should be installed");

    // ── 4. Read a commentary chapter ────────────────────────────
    println!("\nReading {COMMENTARY} on Genesis 1…");
    let chapter = reader
        .read_commentary_chapter(COMMENTARY.into(), "GEN".into(), 1)
        .expect("read_commentary_chapter failed");

    let Some(chapter) = chapter else {
        println!("  no commentary for GEN 1");
        return;
    };

    println!("  book:    {}", chapter.book_name);
    println!("  chapter: {}", chapter.chapter_number);
    println!("  blocks:  {}", chapter.blocks.len());

    for block in chapter.blocks.iter().take(5) {
        let preview: String = block.text.chars().take(120).collect();
        println!("  [verse {:>3}] {}", block.verse_number, preview);
    }
    if chapter.blocks.len() > 5 {
        println!("  … {} more", chapter.blocks.len() - 5);
    }

    assert!(
        !chapter.blocks.is_empty(),
        "GEN 1 should have commentary blocks"
    );

    // ── 5. Uninstall ────────────────────────────────────────────
    println!("\nUninstalling {COMMENTARY}…");
    let task_id = store
        .uninstall_commentary(COMMENTARY.into())
        .expect("commentary uninstall start failed");
    wait_for_task(&store, &task_id);

    let installed_after = reader
        .is_commentary_installed(COMMENTARY.into())
        .expect("is_commentary_installed failed");
    println!("\nAfter uninstall:");
    println!("  installed: {}", installed_after);
    assert!(!installed_after, "{COMMENTARY} should be uninstalled");

    drop(engine);
    cleanup(&db_path);
    println!("\nDone.");
}

// ── Helpers (identical to the translation smoke test) ───────────

fn wait_for_task(
    store: &std::sync::Arc<xbible_engine::engines::freebible_engine::store_api::FreeBibleStore>,
    task_id: &str,
) {
    let mut last_pct = -1.0f64;
    loop {
        thread::sleep(Duration::from_millis(300));
        let Some(status) = store.poll_task(task_id.to_string()) else {
            println!("  task disappeared");
            break;
        };
        let pct = status.progress.fraction * 100.0;
        if pct - last_pct >= 10.0 || pct >= 100.0 {
            last_pct = pct;
            println!("  [{:>5.1}%] {}", pct, status.progress.message);
        }
        match status.state {
            FreeBibleTaskState::Running | FreeBibleTaskState::Queued => continue,
            FreeBibleTaskState::Completed => break,
            FreeBibleTaskState::Failed { error } => panic!("task failed: {error}"),
            FreeBibleTaskState::Cancelled => panic!("task cancelled"),
        }
    }
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path.to_string_lossy()));
    let _ = std::fs::remove_file(format!("{}-shm", path.to_string_lossy()));
}