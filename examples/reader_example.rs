use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use log::{debug, error, info, warn};
use xbible_engine::XBibleEngine;
use xbible_engine::features::reader::SearchProgressListener;
use xbible_engine::features::reader::search::{
    SearchFilter, SearchMode, SearchProgress, SearchResults,
};
use xbible_engine::features::store::{Store, StoreItem, StoreTaskState};

struct LogSearchProgress;

impl SearchProgressListener for LogSearchProgress {
    fn on_progress(&self, progress: SearchProgress) {
        debug!(
            "  [search {:?} {:.0}%] {}",
            progress.phase,
            progress.fraction * 100.0,
            progress.message
        );
    }
}

fn remove_sqlite_file_and_sidecars(db_path: &Path) {
    let base: String = db_path.to_string_lossy().into_owned();
    for candidate in [
        PathBuf::from(&base),
        PathBuf::from(format!("{base}-wal")),
        PathBuf::from(format!("{base}-shm")),
        PathBuf::from(format!("{base}-journal")),
    ] {
        if candidate.exists() {
            if let Err(err) = std::fs::remove_file(&candidate) {
                error!("Warning: could not remove {}: {err}", candidate.display());
            }
        }
    }
}

fn fresh_store_db_path() -> PathBuf {
    let dir: PathBuf = std::env::temp_dir().join("xbible_reader_test");
    if let Err(err) = std::fs::create_dir_all(&dir) {
        error!(
            "Warning: could not create temp dir {}: {err}",
            dir.display()
        );
    }
    let db_path: PathBuf = dir.join("reader_test.db");
    remove_sqlite_file_and_sidecars(&db_path);
    db_path
}

fn poll_until_done(store: &Arc<Store>, task_id: String) -> Result<(), String> {
    let mut last_line: Option<String> = None;

    loop {
        let status = store.poll_task(task_id.clone());
        match status {
            Some(s) => match s.state {
                StoreTaskState::Queued | StoreTaskState::Running => {
                    let line = format!(
                        "  [{:.0}%] {}",
                        s.progress.fraction * 100.0,
                        s.progress.message
                    );
                    if last_line.as_ref() != Some(&line) {
                        info!("{line}");
                        last_line = Some(line);
                    }
                }
                StoreTaskState::Completed => {
                    info!("  Completed: {}", s.progress.message);
                    return Ok(());
                }
                StoreTaskState::Failed { error } => {
                    return Err(format!("Task '{task_id}' failed: {error}"));
                }
                StoreTaskState::Cancelled => {
                    return Err(format!("Task '{task_id}' cancelled"));
                }
            },
            None => return Err(format!("Task '{task_id}' disappeared")),
        }

        thread::sleep(Duration::from_millis(250));
    }
}

fn pick_translation(store: &Arc<Store>, explicit_id: Option<&str>) -> Result<StoreItem, String> {
    if let Some(requested) = explicit_id {
        if let Some(found) = store
            .search(requested.to_string())
            .into_iter()
            .find(|item: &StoreItem| item.id.eq_ignore_ascii_case(requested))
        {
            return Ok(found);
        }
    }

    for candidate_id in ["BSB", "WEB", "ASV", "KJV", "YLT", "DBY"] {
        if let Some(found) = store
            .search(candidate_id.to_string())
            .into_iter()
            .find(|item: &StoreItem| item.id.eq_ignore_ascii_case(candidate_id))
        {
            return Ok(found);
        }
    }

    store
        .search("".to_string())
        .into_iter()
        .find(|item: &StoreItem| item.kind == "translation" || item.kind == "bible")
        .ok_or_else(|| "no translation found in store catalog".to_string())
}

fn log_search_results(label: &str, results: &SearchResults) {
    info!(
        "[{label}] query={:?}: {} total hits (showing {}) in {}ms",
        results.query,
        results.total_hits,
        results.hits.len(),
        results.took_ms
    );

    for hit in results.hits.iter().take(5) {
        let highlighted: Vec<String> = hit
            .highlights
            .iter()
            .filter_map(|span| {
                let s = span.start as usize;
                let e = span.end as usize;
                if s <= e
                    && e <= hit.text.len()
                    && hit.text.is_char_boundary(s)
                    && hit.text.is_char_boundary(e)
                {
                    Some(hit.text[s..e].to_string())
                } else {
                    None
                }
            })
            .collect();

        if highlighted.is_empty() {
            info!(
                "  - [{}] {} {}:{} (score={:.2}): {}",
                hit.kind, hit.book_id, hit.chapter, hit.verse, hit.score, hit.text
            );
        } else {
            info!(
                "  - [{}] {} {}:{} (score={:.2}, matches={:?}): {}",
                hit.kind, hit.book_id, hit.chapter, hit.verse, hit.score, highlighted, hit.text
            );
        }
    }
}

fn run_searches(
    reader: &Arc<xbible_engine::features::reader::XBibleReader>,
    translation_id: &str,
    book_id: &str,
    query: &str,
) -> Result<(), String> {
    // 1. Multi-word search constrained to the target book
    let multi_res = reader
        .search(
            query.to_string(),
            Some(translation_id.to_string()),
            20,
            SearchFilter {
                book_ids: vec![book_id.to_string()],
                mode: SearchMode::MultiWord,
            },
            Some(Box::new(LogSearchProgress)),
        )
        .map_err(|e| format!("multi-word search failed: {e:?}"))?;
    log_search_results("multi_word", &multi_res);

    // 2. Exact phrase search across all books in the translation
    let phrase_res = reader
        .search(
            query.to_string(),
            Some(translation_id.to_string()),
            10,
            SearchFilter {
                book_ids: Vec::new(),
                mode: SearchMode::Phrase,
            },
            Some(Box::new(LogSearchProgress)),
        )
        .map_err(|e| format!("phrase search failed: {e:?}"))?;
    log_search_results("phrase", &phrase_res);

    // 3. Regex search across the translation
    let regex_pattern = format!(
        r"\b{}\w*\b",
        regex::escape(query.split_whitespace().next().unwrap_or(query))
    );
    let regex_res = reader
        .search(
            regex_pattern,
            Some(translation_id.to_string()),
            10,
            SearchFilter {
                book_ids: Vec::new(),
                mode: SearchMode::Regex,
            },
            Some(Box::new(LogSearchProgress)),
        )
        .map_err(|e| format!("regex search failed: {e:?}"))?;
    log_search_results("regex", &regex_res);

    // 4. Direct reference lookup
    let ref_query = format!("{book_id} 3:16");
    let ref_res = reader
        .search(
            ref_query,
            Some(translation_id.to_string()),
            10,
            SearchFilter::default(),
            Some(Box::new(LogSearchProgress)),
        )
        .map_err(|e| format!("reference search failed: {e:?}"))?;
    log_search_results("reference", &ref_res);

    // 5. Strong's / lexicon lookup (if lexicon entries are present)
    let strongs_res = reader
        .search(
            "G26".to_string(),
            Some(translation_id.to_string()),
            10,
            SearchFilter::default(),
            Some(Box::new(LogSearchProgress)),
        )
        .map_err(|e| format!("strongs search failed: {e:?}"))?;
    log_search_results("strongs", &strongs_res);

    Ok(())
}

fn install_and_read(
    engine: &Arc<XBibleEngine>,
    explicit_bible_id: Option<&str>,
    book_id: &str,
    chapter_num: i32,
    query: &str,
) -> Result<(), String> {
    let store: Arc<Store> = engine.store();

    store
        .refresh()
        .map_err(|e| format!("refresh failed: {e:?}"))?;

    let bible: StoreItem = pick_translation(&store, explicit_bible_id)?;
    let translation_id: String = bible.id.clone();

    debug!("Installing '{}' ({})", bible.name, bible.id);
    let bible_task: String = store
        .install(bible)
        .map_err(|e| format!("bible install start failed: {e:?}"))?;
    poll_until_done(&store, bible_task)?;

    let reader = engine.reader();

    for t in reader.get_installed_translations() {
        info!("Installed translation: {} ({})", t.name, t.id);
    }

    for section in reader.get_books(translation_id.clone()) {
        for book in section.books {
            if book.id.eq_ignore_ascii_case(book_id) {
                info!(
                    "Book: {} ({}) — {} chapters",
                    book.name, book.id, book.number_of_chapters
                );
            }
        }
    }

    if let Some(chapter) =
        reader.read_chapter(translation_id.clone(), book_id.to_string(), chapter_num)
    {
        info!(
            "=== {} {} (prev: {:?}, next: {:?}) ===",
            chapter.book_name, chapter.chapter_number, chapter.prev_chapter, chapter.next_chapter
        );

        for block in &chapter.blocks {
            for verse in &block.verses {
                let mut line = format!("[{}] ", verse.number);
                for span in &verse.spans {
                    if let Some(lex) = &span.lexical {
                        line.push_str(&format!(
                            "{}<{}:lemma={}> ",
                            span.text,
                            lex.strongs.as_deref().unwrap_or("?"),
                            lex.lemma.as_deref().unwrap_or("?"),
                        ));
                    } else {
                        line.push_str(&span.text);
                    }
                }
                info!("{line}");
            }
        }
    } else {
        warn!("Chapter {book_id} {chapter_num} not found in {translation_id}");
    }

    run_searches(&reader, &translation_id, book_id, query)?;

    Ok(())
}

fn main() {
    let engine: Arc<XBibleEngine> = XBibleEngine::new();
    let db_path: PathBuf = fresh_store_db_path();
    info!("Using fresh temporary database at: {}", db_path.display());

    let db_path_str: String = db_path.to_string_lossy().into_owned();
    if let Err(err) = engine.set_db_path(db_path_str) {
        error!("Failed to open database {}: {err:?}", db_path.display());
        std::process::exit(1);
    }

    let mut args = env::args().skip(1);
    let explicit_bible_id: Option<String> = args.next();
    let book_id: String = args.next().unwrap_or_else(|| "JHN".to_string());
    let chapter_num: i32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(3);
    let query: String = args.next().unwrap_or_else(|| "love".to_string());

    if let Err(err) = install_and_read(
        &engine,
        explicit_bible_id.as_deref(),
        &book_id,
        chapter_num,
        &query,
    ) {
        error!("Error: {err}");
        std::process::exit(1);
    }
}
