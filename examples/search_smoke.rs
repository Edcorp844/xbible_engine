//! Install a translation, then exercise the search API end to end.
//!
//! Eight queries, one per code path:
//!   1. "shepherd"           → text search (FTS5), single token
//!   2. "love God"           → text search (FTS5), multi-word AND
//!   3. "JHN 3:16"           → reference (single verse, id resolution)
//!   4. "gen 1:1-5"          → reference (verse range, name resolution)
//!   5. "H430"               → lexicon (0 hits until a lexicon is installed)
//!   6. "God" filtered to Psalms → text search with book filter
//!   7. "love God" phrase    → phrase mode (adjacent only)
//!   8. "LORD" regex         → regex mode (bypasses FTS5)
//!
//! Progress events are printed as they arrive, so the phase
//! transition Classifying → Resolving → Querying → Ranking → Done
//! is visible for each query. Highlight spans are printed as the
//! slice of `text` they cover, which is the fastest way to confirm
//! they point at the right bytes.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use log::{error, info};
use xbible_engine::XBibleEngine;
use xbible_engine::engines::freebible_engine::reader::SearchProgressListener;
use xbible_engine::engines::freebible_engine::search::{
    SearchFilter, SearchMode, SearchPhase, SearchProgress, SearchResults,
};
use xbible_engine::engines::freebible_engine::utils::FreeBibleTaskState;

const TRANSLATION: &str = "BSB";

fn main() {
    let db_path = std::env::temp_dir().join(format!("search_smoke_{}.db", std::process::id()));
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

    // ── 2. Run the queries ──────────────────────────────────────
    //
    // Each entry is (label, query, translation_id, filter).
    // The label is only for the section header so it's clear which
    // code path each query exercises.
    let queries: &[(&str, &str, Option<&str>, SearchFilter)] = &[
        // Text search (FTS5), single token.
        (
            "text, single token",
            "shepherd",
            Some(TRANSLATION),
            SearchFilter::default(),
        ),
        // Text search (FTS5), multi-word AND (the default mode).
        (
            "text, multi-word",
            "love God",
            Some(TRANSLATION),
            SearchFilter::default(),
        ),
        // Reference, single verse, id resolution.
        (
            "reference, id",
            "JHN 3:16",
            Some(TRANSLATION),
            SearchFilter::default(),
        ),
        // Reference, verse range, name resolution.
        (
            "reference, range",
            "gen 1:1-5",
            Some(TRANSLATION),
            SearchFilter::default(),
        ),
        // Lexicon — 0 hits until a lexicon dataset is installed.
        ("lexicon", "H430", None, SearchFilter::default()),
        // Text search restricted to Psalms.
        (
            "text, filtered to PSA",
            "God",
            Some(TRANSLATION),
            SearchFilter {
                book_ids: vec!["PSA".into()],
                ..SearchFilter::default()
            },
        ),
        // Phrase mode: "love God" must be adjacent, not just both
        // tokens present. Compare the hit count against the
        // multi-word query above — phrase mode should return fewer.
        (
            "text, phrase mode",
            "love God",
            Some(TRANSLATION),
            SearchFilter {
                mode: SearchMode::Phrase,
                ..SearchFilter::default()
            },
        ),
        // Regex mode: `\bLord\b` matches the whole word "Lord" and
        // "LORD" (case-insensitive), but not "Lordship". Bypasses
        // FTS5 entirely — scans verse text with the compiled regex.
        (
            "text, regex mode",
            r"(?i)\bLord\b",
            Some(TRANSLATION),
            SearchFilter {
                mode: SearchMode::Regex,
                ..SearchFilter::default()
            },
        ),
    ];

    for (label, query, tid, filter) in queries {
        info!("\n════════════════════════════════════════");
        info!("[{label}]");
        info!("Query: {query:?}   translation={tid:?}   filter={filter:?}");
        info!("────────────────────────────────────────");

        let phases = Arc::new(Mutex::new(Vec::<SearchPhase>::new()));
        let listener = PrintingListener {
            phases: phases.clone(),
        };

        let result: Result<SearchResults, _> = reader.search(
            (*query).to_string(),
            tid.map(|s| s.to_string()),
            20,
            filter.clone(),
            Some(Box::new(listener)),
        );

        match result {
            Ok(results) => {
                info!(
                    "  → {} hit(s) in {} ms  (total: {})",
                    results.hits.len(),
                    results.took_ms,
                    results.total_hits
                );
                for (i, hit) in results.hits.iter().take(5).enumerate() {
                    let preview: String = hit.text.chars().take(90).collect();
                    let preview = preview.replace('\n', " ");
                    info!(
                        "    {}. [{}] {} {}:{}  (score {:.2}, {} highlight(s))",
                        i + 1,
                        hit.kind,
                        hit.book_name,
                        hit.chapter,
                        hit.verse,
                        hit.score,
                        hit.highlights.len(),
                    );
                    info!("       {preview}");

                    if let Some(h) = hit.highlights.first() {
                        let start = (h.start as usize).min(hit.text.len());
                        let end = (h.end as usize).min(hit.text.len());
                        if start < end {
                            info!("       first match: {:?}", &hit.text[start..end]);
                        }
                    }
                }
                if results.hits.len() > 5 {
                    info!("    … {} more", results.hits.len() - 5);
                }
            }
            Err(e) => error!("  → error: {e:?}"),
        }

        let collected = phases.lock().unwrap_or_else(|p| p.into_inner());
        info!(
            "  phases seen: {:?}",
            collected
                .iter()
                .map(|p| format!("{p:?}"))
                .collect::<Vec<_>>()
        );
    }

    // ── 3. Cleanup ──────────────────────────────────────────────
    drop(engine);
    cleanup(&db_path);
    info!("\nDone.");
}

// ── Progress listener ──────────────────────────────────────────

struct PrintingListener {
    phases: Arc<Mutex<Vec<SearchPhase>>>,
}

impl SearchProgressListener for PrintingListener {
    fn on_progress(&self, progress: SearchProgress) {
        info!(
            "  [{:>4.0}%] {:?}: {}",
            progress.fraction * 100.0,
            progress.phase,
            progress.message
        );
        self.phases
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(progress.phase);
    }
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
