//! Install a commentary, read a chapter from it, uninstall it.
//!
//! Commentary endpoints use the `/c/` prefix, not the translation prefix:
//!   /api/c/{commentary}/books.json
//!   /api/c/{commentary}/{book}/{chapter}.json
//!
//! Commentaries have no `complete.json`, so the install is per-chapter
//! (parallel), the same fallback path translations use when
//! `complete.json` is unavailable.

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

    // The catalog row for a commentary is what `install_commentary`
    // will later flip to `installed = 1`. If it's missing, the
    // install succeeds and `mark_installed` fails with "not in
    // catalog" — the same trap the translation install had.
    println!("Catalog refreshed. Looking for {COMMENTARY}…");

    // ── 2. Install the commentary ───────────────────────────────
    //
    // REQUIRES: `FreeBibleStore::install_commentary(&self, commentary_id: String) -> Result<String, FreeBibleError>`
    //
    // That method does not exist yet. It should:
    //   - look up the commentary in `commentaries` (error if absent)
    //   - spawn a task thread (like `install_translation`)
    //   - in the task: `client.commentary_books(id)?` → Vec<FreeBibleBook>
    //   - submit `begin_commentary_install(conn, id)` to the writer
    //   - for each book, for each chapter: fetch and persist
    //     (commentaries have no complete.json; this is the parallel path)
    //   - submit `mark_commentary_installed(conn, id)` last
    //
    // The SQL is identical to the translation install, but the
    // content lands under the commentary_id in the same
    // `chapter_items` / `chapter_raw` tables, with a NULL
    // `translation_id` and a separate `commentary_id` — or, simpler,
    // reuse `translation_id` for the commentary id and add a
    // `kind` column in a future migration. See the note at the end.
    println!("\nInstalling commentary {COMMENTARY}…");
    let task_id = store
        .install_translation(COMMENTARY.into())
        .expect("commentary install start failed");
    wait_for_task(&store, &task_id);

    // ── 3. Verify the install landed ────────────────────────────
    let installed = reader
        .is_installed(COMMENTARY.into())
        .expect("is_commentary_installed failed");
    println!("\nAfter install:");
    println!("  installed: {}", installed);
    assert!(installed, "{COMMENTARY} should be installed");

    // ── 4. Read a commentary chapter ────────────────────────────
    //
    // REQUIRES: `FreeBibleReader::read_commentary_chapter(&self, commentary_id: String, book_id: String, chapter: i32) -> Result<Option<CommentaryChapter>, FreeBibleError>`
    //
    // That method does not exist yet. It should query the same
    // `chapter_items` / `footnotes` tables the translation reader
    // uses, filtered by the commentary id, and return a shape
    // similar to `ReaderChapter` but with `commentary_id` instead
    // of `translation_id`. For Adam Clarke, the "verses" are
    // commentary paragraphs keyed to a verse number, so the natural
    // shape is `Vec<CommentaryBlock { verse_number: i32, text: String }>`.
    println!("\nReading {COMMENTARY} on Genesis 1…");
    let chapter = reader
        .read_chapter(COMMENTARY.into(), "GEN".into(), 1)
        .expect("read_commentary_chapter failed");

    println!("  book:    {}", chapter.book_name);
    println!("  chapter: {}", chapter.chapter_number);
    println!("  blocks:  {}", chapter.blocks.len());

    for block in chapter.blocks.iter().take(5) {
        let preview: String = block.text.chars().take(120).collect();
        if let Some(verse_number) = block.verse_number {
            println!(
                "  [verse {:>3}] {}",
                verse_number,
                preview
            );
        } else {
            println!("  [no verse] {}", preview);
        }
    }
    if chapter.blocks.len() > 5 {
        println!("  … {} more", chapter.blocks.len() - 5);
    }

    assert!(
        !chapter.blocks.is_empty(),
        "GEN 1 should have commentary blocks"
    );

    // ── 5. Uninstall ────────────────────────────────────────────
    //
    // REQUIRES: `FreeBibleStore::uninstall_commentary(&self, commentary_id: String) -> Result<String, FreeBibleError>`
    println!("\nUninstalling {COMMENTARY}…");
    let task_id = store
        .uninstall_translation(COMMENTARY.into())
        .expect("commentary uninstall start failed");
    wait_for_task(&store, &task_id);

    let installed_after = reader
        .is_installed(COMMENTARY.into())
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