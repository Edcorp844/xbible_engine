use std::env;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use xbible_engine::XBibleEngine;
use xbible_engine::features::reader::XBibleReader;
use xbible_engine::features::store::{Store, StoreItem, StoreTaskState};

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
                eprintln!("Warning: could not remove {}: {err}", candidate.display());
            }
        }
    }
}

fn fresh_store_db_path() -> PathBuf {
    let dir: PathBuf = std::env::temp_dir().join("xbible_reader_test");
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!(
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
                        println!("{line}");
                        last_line = Some(line);
                    }
                }
                StoreTaskState::Completed => {
                    println!("  Completed: {}", s.progress.message);
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

fn print_chapters(
    reader: &Arc<XBibleReader>,
    translation_id: &str,
    book_id: &str,
    chapters: &[i32],
) {
    for &chapter_num in chapters {
        match reader.read_chapter(translation_id.to_string(), book_id.to_string(), chapter_num) {
            Some(chapter) => {
                println!(
                    "\n=== {} ({}) Chapter {} (blocks: {}) ===",
                    chapter.book_name,
                    book_id,
                    chapter.chapter_number,
                    chapter.blocks.len()
                );

                let mut verse_count = 0usize;
                let mut empty_verse_count = 0usize;

                for (block_idx, block) in chapter.blocks.iter().enumerate() {
                    for verse in &block.verses {
                        verse_count += 1;
                        let mut text = String::new();
                        for span in &verse.spans {
                            text.push_str(&span.text);
                        }
                        if text.trim().is_empty() {
                            empty_verse_count += 1;
                        }
                        println!(
                            "  [block {} | v{}] (spans={}) {:?}",
                            block_idx,
                            verse.number,
                            verse.spans.len(),
                            text
                        );
                    }
                }

                println!(
                    "Summary {} {}: {} verses total, {} empty",
                    book_id, chapter_num, verse_count, empty_verse_count
                );
            }
            None => {
                println!("\n=== {book_id} Chapter {chapter_num}: NOT FOUND ===");
            }
        }
    }
}

fn audit_all_books(reader: &Arc<XBibleReader>, translation_id: &str) {
    println!("\n================ BOOK AUDIT ({translation_id}) ================");
    for section in reader.get_books(translation_id.to_string()) {
        for book in section.books {
            let ch1 = reader.read_chapter(translation_id.to_string(), book.id.clone(), 1);
            let (total_v, empty_v) = match ch1 {
                Some(ch) => {
                    let mut total = 0;
                    let mut empty = 0;
                    for b in &ch.blocks {
                        for v in &b.verses {
                            total += 1;
                            let combined: String =
                                v.spans.iter().map(|s| s.text.as_str()).collect();
                            if combined.trim().is_empty() {
                                empty += 1;
                            }
                        }
                    }
                    (total, empty)
                }
                None => (0, 0),
            };

            println!(
                "  {:6} ({:<20}) chs={:<3} | ch1: total={:<3} empty={:<3} {}",
                book.id,
                book.name,
                book.number_of_chapters,
                total_v,
                empty_v,
                if total_v > 0 && empty_v == total_v {
                    "<-- ALL EMPTY"
                } else if empty_v > 0 {
                    "<-- PARTIAL EMPTY"
                } else if total_v == 0 {
                    "<-- NO CH1"
                } else {
                    "OK"
                }
            );
        }
    }
}

fn main() {
    let engine: Arc<XBibleEngine> = XBibleEngine::new();
    let db_path: PathBuf = fresh_store_db_path();
    println!("Using fresh temporary database at: {}", db_path.display());

    if let Err(err) = engine.set_db_path(db_path.to_string_lossy().into_owned()) {
        eprintln!("Failed to open database {}: {err:?}", db_path.display());
        std::process::exit(1);
    }

    let explicit_bible_id: Option<String> = env::args().nth(1);
    let store: Arc<Store> = engine.store();

    if let Err(err) = store.refresh() {
        eprintln!("Refresh failed: {err:?}");
        std::process::exit(1);
    }

    let bible = match pick_translation(&store, explicit_bible_id.as_deref()) {
        Ok(b) => b,
        Err(err) => {
            eprintln!("Pick translation failed: {err}");
            std::process::exit(1);
        }
    };

    let translation_id = bible.id.clone();
    println!("Installing '{}' ({})...", bible.name, translation_id);

    let task_id = match store.install(bible) {
        Ok(id) => id,
        Err(err) => {
            eprintln!("Install failed: {err:?}");
            std::process::exit(1);
        }
    };

    if let Err(err) = poll_until_done(&store, task_id) {
        eprintln!("{err}");
        std::process::exit(1);
    }

    let reader = engine.reader();

    // 1. Audit Chapter 1 of every book in the translation
    audit_all_books(&reader, &translation_id);

    // 2. Print Psalms 1-2 and Job 1-2 specifically
    print_chapters(&reader, &translation_id, "PSA", &[1, 2]);
    print_chapters(&reader, &translation_id, "JOB", &[1, 2]);
}
