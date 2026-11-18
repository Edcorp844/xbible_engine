//! Install a translation, then verify that `get_books` returns the
//! books grouped by canonical section, with the correct per-section
//! counts and the correct `is_apocryphal` classification.
//!
//! This example exercises the install → persist → group path end to
//! end. The install downloads `complete.json`, parses it, and calls
//! `persist_book` per book. `persist_book` computes each book's
//! `book_metadata.section` from `section_for_book(book_id,
//! is_apocryphal)`. `get_books` then groups by that column and
//! returns one `BookSection` per canonical group, in order:
//!
//!   ot → nt → deutero → orthodox_extra → ethiopian_extra → unknown
//!
//! Empty groups are dropped. A Protestant translation returns two
//! sections; a Catholic or Orthodox translation returns three or
//! four; a translation with extras returns one per extra category.
//!
//! The exact counts depend on the translation's canon:
//!
//!   eng_kja  (KJV + Apocrypha)     39 / 27 / 14
//!   eng_dra  (Douay-Rheims 1899)   39 / 27 /  7
//!   BSB      (Protestant)          39 / 27 /  —
//!
//! The `is_apocryphal` flag comes from `books.json` — the
//! authoritative endpoint — and is merged into each book inside
//! `client.complete()` before `persist_book` sees it. A pre-v8
//! install can be fixed in place via `rebuild_sections` without
//! re-downloading.

use std::collections::BTreeMap;
use std::thread;
use std::time::Duration;

use log::info;
use xbible_engine::XBibleEngine;
use xbible_engine::engines::freebible_engine::utils::FreeBibleTaskState;

/// Translation to install. Change this and the matching
/// `expected_sections` to test a different canon shape.
const TRANSLATION: &str = "eng_dra";

/// Expected `(ot, nt, deutero)` book counts for `TRANSLATION`.
/// `None` means "the section should not exist" — used for
/// translations that have no Deuterocanonicals at all.
const EXPECTED: (usize, usize, Option<usize>) = (39, 27, Some(7));

fn main() {
    let db_path = std::env::temp_dir().join(format!("install_smoke_{}.db", std::process::id()));
    cleanup(&db_path);

    let engine = XBibleEngine::new();
    engine
        .freebible()
        .set_db_path(db_path.to_string_lossy().into_owned())
        .expect("engine open failed");

    let store = engine.freebible().store();
    let reader = engine.freebible().reader();

    // ── 1. Catalog + install ────────────────────────────────────
    info!("Refreshing catalog…");
    store.refresh().expect("refresh failed");

    info!("\nInstalling {TRANSLATION}…");
    let task_id = store
        .install_translation(TRANSLATION.into())
        .expect("install start failed");
    wait_for_task(&store, &task_id);

    info!(
        "\nInstalled. {}",
        reader
            .is_installed(TRANSLATION.into())
            .map(|v| format!("is_installed = {v}"))
            .unwrap_or_else(|e| format!("is_installed error: {e:?}"))
    );

    // ── 2. Discover sections from the installed translation ─────
    //
    // `get_books` returns every book the translation has, grouped
    // by canonical section. No canon filtering — `get_books` is a
    // view over `book_metadata.section`, and the classification
    // comes from `is_apocryphal` at install time.
    let sections_list = reader.get_books(TRANSLATION.into());
    let total_books: usize = sections_list.iter().map(|s| s.books.len()).sum();
    info!(
        "\n{} book(s) across {} section(s) installed",
        total_books,
        sections_list.len()
    );

    let mut sections: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for section in &sections_list {
        let ids: Vec<String> = section.books.iter().map(|b| b.id.clone()).collect();
        sections.insert(section.section.clone(), ids);
    }
    for (section, ids) in &sections {
        info!("  section {section:?}: {} book(s)", ids.len());
    }

    // ── 3. Verify the per-section counts ────────────────────────
    let (want_ot, want_nt, want_deutero) = EXPECTED;

    assert_eq!(
        sections.get("ot").map(|v| v.len()),
        Some(want_ot),
        "expected {want_ot} OT books for {TRANSLATION}"
    );
    assert_eq!(
        sections.get("nt").map(|v| v.len()),
        Some(want_nt),
        "expected {want_nt} NT books for {TRANSLATION}"
    );
    match want_deutero {
        Some(n) => assert_eq!(
            sections.get("deutero").map(|v| v.len()),
            Some(n),
            "expected {n} Deuterocanonical books for {TRANSLATION}"
        ),
        None => assert!(
            sections.get("deutero").is_none(),
            "expected no Deuterocanonical section for {TRANSLATION}, \
             but found {} books",
            sections.get("deutero").map(|v| v.len()).unwrap_or(0),
        ),
    }

    // ── 4. Verify each book's section matches its container ─────
    //
    // The `FreeBibleBook.section` field must agree with the section
    // key it was grouped under. This catches a class of bugs where
    // `section_for_book` produces one value but `get_books` groups
    // by another (e.g. a typo in the match arm).
    for section in &sections_list {
        for book in &section.books {
            assert_eq!(
                book.section, section.section,
                "book {} is in section {:?} but its `section` field says {:?}",
                book.id, section.section, book.section,
            );
        }
    }

    // ── 5. Verify every book appears exactly once ───────────────
    //
    // If `get_books`'s grouping logic ever duplicated a book across
    // two sections (e.g. a `BTreeMap` insert bug), this catches it.
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for section in &sections_list {
        for book in &section.books {
            assert!(
                seen.insert(&book.id),
                "book {} appears more than once across sections",
                book.id,
            );
        }
    }
    assert_eq!(
        seen.len(),
        total_books,
        "unique book count ({}) != total book count ({})",
        seen.len(),
        total_books,
    );

    info!("\nAll section assertions passed for {TRANSLATION}.");

    // ── 6. Cleanup ──────────────────────────────────────────────
    drop(engine);
    cleanup(&db_path);
    info!("\nDone.");
}

// ── Task polling ──────────────────────────────────────────────

fn wait_for_task(
    store: &std::sync::Arc<xbible_engine::engines::freebible_engine::store_api::FreeBibleStore>,
    task_id: &str,
) {
    let mut last_pct = -1.0f64;
    loop {
        thread::sleep(Duration::from_millis(300));
        let Some(status) = store.poll_task(task_id.to_string()) else {
            info!("  task disappeared");
            break;
        };
        let pct = status.progress.fraction * 100.0;
        if pct - last_pct >= 10.0 || pct >= 100.0 {
            last_pct = pct;
            info!("  [{:>5.1}%] {}", pct, status.progress.message);
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
