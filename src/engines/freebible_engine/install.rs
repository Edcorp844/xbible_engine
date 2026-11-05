use rusqlite::{params, Connection};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use log::{error, info, warn};

use crate::engines::freebible_engine::client::{BibleClient, FreeBibleChapter, FreeBibleComplete};
use crate::engines::freebible_engine::utils::{FreeBibleError, InstallProgress};

const WORKERS: usize = 8;
const CHANNEL_CAPACITY: usize = 16;

/// Every table that stores per-translation content. Used for wipe-before-install
/// and for uninstall so no orphan rows survive.
const CONTENT_TABLES: &[&str] = &[
    "verses",
    "books",
    "book_metadata",
    "chapter_items",
    "footnotes",
    "audio_timings",
    "chapter_raw",
];

fn wipe_translation(
    tx: &rusqlite::Transaction<'_>,
    translation_id: &str,
) -> Result<(), FreeBibleError> {
    for table in CONTENT_TABLES {
        tx.execute(
            &format!("DELETE FROM {table} WHERE translation_id = ?1"),
            params![translation_id],
        )
        .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
    }
    Ok(())
}

/// Download and persist a whole translation.
///
/// Primary path: one `complete.json` request (fast, full fidelity).
/// Fallback: parallel per-chapter fetches, used only when `complete.json`
/// isn't available (NT-only translations, or partial modules).
pub fn install_translation(
    client: &BibleClient,
    conn: &mut Connection,
    translation_id: &str,
    progress: impl Fn(InstallProgress),
    cancel: Arc<AtomicBool>,
) -> Result<String, FreeBibleError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(FreeBibleError::Generic {
            error_message: "cancelled".into(),
        });
    }

    let download_progress = |received: u64, total: Option<u64>| {
        let fraction = match total {
            Some(t) if t > 0 => (received as f64 / t as f64) * 0.8,
            _ => 0.0,
        };
        progress(InstallProgress {
            fraction,
            chapters_done: 0,
            chapters_total: 0,
            current_book_name: String::new(),
            current_book_id: String::new(),
            current_chapter: 0,
            current_chapter_verses: 0,
            verses_done: 0,
            message: match total {
                Some(t) => format!(
                    "Downloading {} — {:.1} / {:.1} MB",
                    translation_id,
                    received as f64 / 1_048_576.0,
                    t as f64 / 1_048_576.0,
                ),
                None => format!(
                    "Downloading {} — {:.1} MB",
                    translation_id,
                    received as f64 / 1_048_576.0,
                ),
            },
        });
    };

    let complete = match client.complete(translation_id, download_progress) {
        Ok(c) => {
            info!(
                "complete.json OK for {}: {} books",
                translation_id,
                c.books.len()
            );
            if let Some(b) = c.books.first() {
                if let Some(ch) = b.chapters.first() {
                    info!(
                        "first chapter: num={} verses={} items={} footnotes={} timings={} raw_json_len={}",
                        ch.number, ch.verses.len(), ch.items.len(),
                        ch.footnotes.len(), ch.audio_timings.len(), ch.raw_json.len()
                    );
                }
            }
            c
        }
        Err(e) => {
            info!("COMPLETE.JSON FAILED for {}: {}", translation_id, e);
            return install_parallel(client, conn, translation_id, progress, cancel);
        }
    };

    install_from_complete(conn, translation_id, complete, progress, cancel)
}

// ── Fast path: complete.json ──────────────────────────────────────

fn install_from_complete(
    conn: &mut Connection,
    translation_id: &str,
    complete: FreeBibleComplete,
    progress: impl Fn(InstallProgress),
    cancel: Arc<AtomicBool>,
) -> Result<String, FreeBibleError> {
    let total_chapters: i32 = complete.books.iter().map(|b| b.chapters.len() as i32).sum();

    let tx = conn
        .transaction()
        .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
    wipe_translation(&tx, translation_id)?;

    // Books + per-book metadata
    {
        let mut book_stmt = tx
            .prepare(
                "INSERT INTO books (translation_id, id, name, number_of_chapters)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
        let mut meta_stmt = tx
            .prepare(
                "INSERT INTO book_metadata (
                    translation_id, book_id, common_name, title,
                    book_order, audio_links_json, raw_json
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            )
            .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;

        for b in &complete.books {
            book_stmt
                .execute(params![translation_id, b.id, b.name, b.number_of_chapters])
                .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
            meta_stmt
                .execute(params![
                    translation_id,
                    b.id,
                    b.common_name,
                    b.title,
                    b.book_order,
                    b.audio_links_json,
                    b.raw_json,
                ])
                .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
        }
    }

    // Chapter-scoped prepared statements
    let mut verse_stmt = tx
        .prepare(
            "INSERT INTO verses (translation_id, book_id, chapter, verse, text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
    let mut item_stmt = tx
        .prepare(
            "INSERT INTO chapter_items (
                translation_id, book_id, chapter, position, item_type,
                verse_number, text, poem, words_of_jesus, footnote_ids, raw_json
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        )
        .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
    let mut footnote_stmt = tx
        .prepare(
            "INSERT INTO footnotes (
                translation_id, book_id, chapter, note_id,
                caller, text, reference, raw_json
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        )
        .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
    let mut timing_stmt = tx
        .prepare(
            "INSERT INTO audio_timings (
                translation_id, book_id, chapter, verse, start_ms, end_ms
             ) VALUES (?1,?2,?3,?4,?5,?6)",
        )
        .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
    let mut raw_stmt = tx
        .prepare(
            "INSERT INTO chapter_raw (translation_id, book_id, chapter, raw_json)
             VALUES (?1,?2,?3,?4)",
        )
        .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;

    let mut chapters_done = 0i32;
    let mut verses_done = 0i32;

    for book in &complete.books {
        for chapter in &book.chapters {
            if cancel.load(Ordering::Relaxed) {
                return Err(FreeBibleError::Generic {
                    error_message: "cancelled".into(),
                });
            }

            for v in &chapter.verses {
                verse_stmt
                    .execute(params![
                        translation_id,
                        book.id,
                        chapter.number,
                        v.number,
                        v.text
                    ])
                    .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
            }

            for item in &chapter.items {
                let footnote_ids = if item.footnote_ids.is_empty() {
                    None
                } else {
                    Some(item.footnote_ids.join(","))
                };
                item_stmt
                    .execute(params![
                        translation_id,
                        book.id,
                        chapter.number,
                        item.position,
                        item.item_type,
                        item.verse_number,
                        item.text,
                        item.poem as i32,
                        item.words_of_jesus as i32,
                        footnote_ids,
                        item.raw_json,
                    ])
                    .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
            }

            for note in &chapter.footnotes {
                footnote_stmt
                    .execute(params![
                        translation_id,
                        book.id,
                        chapter.number,
                        note.note_id,
                        note.caller,
                        note.text,
                        note.reference,
                        note.raw_json,
                    ])
                    .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
            }

            for timing in &chapter.audio_timings {
                timing_stmt
                    .execute(params![
                        translation_id,
                        book.id,
                        chapter.number,
                        timing.verse,
                        timing.start_ms,
                        timing.end_ms,
                    ])
                    .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
            }

            raw_stmt
                .execute(params![
                    translation_id,
                    book.id,
                    chapter.number,
                    chapter.raw_json
                ])
                .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;

            chapters_done += 1;
            verses_done += chapter.verses.len() as i32;

            let fraction = 0.8 + (chapters_done as f64 / total_chapters as f64) * 0.2;

            progress(InstallProgress {
                fraction,
                chapters_done,
                chapters_total: total_chapters,
                current_book_name: book.name.clone(),
                current_book_id: book.id.clone(),
                current_chapter: chapter.number,
                current_chapter_verses: chapter.verses.len() as i32,
                verses_done,
                message: format!(
                    "{} {} — {} verses  •  {}/{} chapters  •  {} verses total",
                    book.name,
                    chapter.number,
                    chapter.verses.len(),
                    chapters_done,
                    total_chapters,
                    verses_done,
                ),
            });
        }
    }

    drop(verse_stmt);
    drop(item_stmt);
    drop(footnote_stmt);
    drop(timing_stmt);
    drop(raw_stmt);

    tx.execute(
        "UPDATE translations SET installed = 1 WHERE id = ?1",
        params![translation_id],
    )
    .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;

    tx.commit()
        .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
    info!(
        "installed {} via complete.json ({} chapters, {} verses)",
        translation_id, chapters_done, verses_done
    );
    Ok(translation_id.to_string())
}

// ── Fallback: parallel per-chapter ────────────────────────────────

fn install_parallel(
    client: &BibleClient,
    conn: &mut Connection,
    translation_id: &str,
    progress: impl Fn(InstallProgress),
    cancel: Arc<AtomicBool>,
) -> Result<String, FreeBibleError> {
    let books = client.books(translation_id)?;
    let total_chapters: i32 = books.iter().map(|b| b.number_of_chapters).sum();

    let mut tasks: Vec<(String, String, i32)> = Vec::with_capacity(total_chapters as usize);
    for book in &books {
        for chapter in 1..=book.number_of_chapters {
            tasks.push((book.id.clone(), book.name.clone(), chapter));
        }
    }
    tasks.reverse();

    let queue = Arc::new(Mutex::new(tasks));
    let shared_client = Arc::new(BibleClient::new());
    let _ = client;
    let (tx, rx) =
        mpsc::sync_channel::<(String, String, i32, FreeBibleChapter)>(CHANNEL_CAPACITY);

    let mut handles = Vec::with_capacity(WORKERS);
    for _ in 0..WORKERS {
        let queue = queue.clone();
        let tx = tx.clone();
        let client = shared_client.clone();
        let tid = translation_id.to_string();
        let cancel = cancel.clone();

        handles.push(thread::spawn(move || loop {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let next = {
                let mut q = queue.lock().unwrap();
                q.pop()
            };
            let Some((book_id, book_name, chapter)) = next else {
                return;
            };

            match client.chapter_simple(&tid, &book_id, chapter as u32) {
                Ok(ch) => {
                    if tx.send((book_id, book_name, chapter, ch)).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    error!("fetch {} {}:{} failed: {}", tid, book_id, chapter, e);
                }
            }
        }));
    }
    drop(tx);

    let result = (|| -> Result<(), FreeBibleError> {
        let db_tx = conn
            .transaction()
            .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
        wipe_translation(&db_tx, translation_id)?;

        {
            let mut stmt = db_tx
                .prepare(
                    "INSERT INTO books (translation_id, id, name, number_of_chapters)
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
            for b in &books {
                stmt.execute(params![translation_id, b.id, b.name, b.number_of_chapters])
                    .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
            }
        }

        let mut verse_stmt = db_tx
            .prepare(
                "INSERT INTO verses (translation_id, book_id, chapter, verse, text)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;

        let mut chapters_done = 0i32;
        let mut verses_done = 0i32;

        while let Ok((book_id, book_name, chapter, ch)) = rx.recv() {
            if cancel.load(Ordering::Relaxed) {
                return Err(FreeBibleError::Generic {
                    error_message: "cancelled".into(),
                });
            }

            for v in &ch.content {
                verse_stmt
                    .execute(params![translation_id, book_id, chapter, v.number, v.text])
                    .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
            }

            chapters_done += 1;
            verses_done += ch.content.len() as i32;
            let fraction = chapters_done as f64 / total_chapters as f64;

            progress(InstallProgress {
                fraction,
                chapters_done,
                chapters_total: total_chapters,
                current_book_name: book_name.clone(),
                current_book_id: book_id.clone(),
                current_chapter: chapter,
                current_chapter_verses: ch.content.len() as i32,
                verses_done,
                message: format!(
                    "{} {} — {} verses  •  {}/{} chapters  •  {} verses total",
                    book_name,
                    chapter,
                    ch.content.len(),
                    chapters_done,
                    total_chapters,
                    verses_done,
                ),
            });
        }

        drop(verse_stmt);

        if cancel.load(Ordering::Relaxed) {
            return Err(FreeBibleError::Generic {
                error_message: "cancelled".into(),
            });
        }

        db_tx
            .execute(
                "UPDATE translations SET installed = 1 WHERE id = ?1",
                params![translation_id],
            )
            .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;

        db_tx
            .commit()
            .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
        Ok(())
    })();

    for h in handles {
        let _ = h.join();
    }

    result.map(|_| translation_id.to_string())
}

// ── Uninstall ─────────────────────────────────────────────────────

pub fn uninstall_translation(
    conn: &mut Connection,
    translation_id: &str,
) -> Result<String, FreeBibleError> {
    let tx = conn
        .transaction()
        .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
    wipe_translation(&tx, translation_id)?;
    tx.execute(
        "UPDATE translations SET installed = 0 WHERE id = ?1",
        params![translation_id],
    )
    .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
    tx.commit()
        .map_err(|e| FreeBibleError::Generic { error_message: e.to_string() })?;
    info!("uninstalled {}", translation_id);
    Ok(translation_id.to_string())
}