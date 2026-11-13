use rusqlite::{Connection, params};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use log::{error, info, warn};

use crate::engines::freebible_engine::client::{
    BibleClient, FreeBibleBook, FreeBibleCommentaryChapter, FreeBibleCompleteBook, FreeBibleVerse,
};
use crate::engines::freebible_engine::error::FreeBibleError;

const WORKERS: usize = 8;
const CHANNEL_CAPACITY: usize = 16;

// ── EntityKind ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    Person,
    Event,
    Place,
    Group,
}

impl EntityKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EntityKind::Person => "person",
            EntityKind::Event => "event",
            EntityKind::Place => "place",
            EntityKind::Group => "group",
        }
    }
    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "person" => Some(Self::Person),
            "event" => Some(Self::Event),
            "place" => Some(Self::Place),
            "group" => Some(Self::Group),
            _ => None,
        }
    }
}

/// Map API `type` values (plural) to our canonical kinds (singular).
fn normalize_kind(api_type: &str) -> &str {
    match api_type {
        "people" => "person",
        "places" => "place",
        "events" => "event",
        "groups" => "group",
        other => other,
    }
}

/// Merge verses that share a verse number within one chapter.
///
/// Some translations (especially non-standard versification) repeat a
/// verse number, e.g. a page break splitting verse 12 into two rows.
/// The `verses` table has a UNIQUE key on
/// `(translation_id, book_id, chapter, verse)`, so duplicates would
/// abort the install. Concatenating preserves all text; the structured
/// `chapter_items` table keeps the unmerged representation.
fn merge_verse_rows(verses: &[FreeBibleVerse]) -> BTreeMap<i32, String> {
    let mut merged: BTreeMap<i32, String> = BTreeMap::new();
    for v in verses {
        merged
            .entry(v.number)
            .and_modify(|existing| {
                if !existing.is_empty() && !v.text.is_empty() {
                    existing.push(' ');
                }
                existing.push_str(&v.text);
            })
            .or_insert_with(|| v.text.clone());
    }
    merged
}

// ── Translation content tables ────────────────────────────────

const CONTENT_TABLES: &[&str] = &[
    "verses",
    "books",
    "book_metadata",
    "chapter_items",
    "footnotes",
    "audio_timings",
    "chapter_raw",
];

pub(crate) fn wipe_translation(
    conn: &Connection,
    translation_id: &str,
) -> Result<(), FreeBibleError> {
    for table in CONTENT_TABLES {
        conn.execute(
            &format!("DELETE FROM {table} WHERE translation_id = ?1"),
            params![translation_id],
        )
        .map_err(|e| FreeBibleError::Generic {
            error_message: e.to_string(),
        })?;
    }
    Ok(())
}

// ── Translation install helpers (called from store_api via writer) ──

pub(crate) fn begin_install(conn: &Connection, translation_id: &str) -> Result<(), FreeBibleError> {
    wipe_translation(conn, translation_id)?;
    conn.execute(
        "UPDATE translations SET installed = 0 WHERE id = ?1",
        params![translation_id],
    )
    .map_err(|e| FreeBibleError::Generic {
        error_message: format!("clear installed flag: {e}"),
    })?;
    Ok(())
}

pub(crate) fn persist_book(
    conn: &Connection,
    translation_id: &str,
    book: &FreeBibleCompleteBook,
) -> Result<(), FreeBibleError> {
    conn.execute(
        "INSERT OR REPLACE INTO books (translation_id, id, name, number_of_chapters)
         VALUES (?1, ?2, ?3, ?4)",
        params![translation_id, book.id, book.name, book.number_of_chapters],
    )
    .map_err(|e| FreeBibleError::Generic {
        error_message: format!("books insert ({}): {e}", book.id),
    })?;

    conn.execute(
        "INSERT OR REPLACE INTO book_metadata (
            translation_id, book_id, common_name, title,
            book_order, audio_links_json, raw_json
         ) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![
            translation_id,
            book.id,
            book.common_name,
            book.title,
            book.book_order,
            book.audio_links_json,
            book.raw_json,
        ],
    )
    .map_err(|e| FreeBibleError::Generic {
        error_message: format!("book_metadata insert ({}): {e}", book.id),
    })?;

    let mut verse_stmt = conn
        .prepare(
            "INSERT OR REPLACE INTO verses (translation_id, book_id, chapter, verse, text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .map_err(|e| FreeBibleError::Generic {
            error_message: format!("prepare verses: {e}"),
        })?;
    let mut item_stmt = conn
        .prepare(
            "INSERT OR REPLACE INTO chapter_items (
                translation_id, book_id, chapter, position, item_type,
                verse_number, text, poem, words_of_jesus, footnote_ids, raw_json
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        )
        .map_err(|e| FreeBibleError::Generic {
            error_message: format!("prepare chapter_items: {e}"),
        })?;
    let mut footnote_stmt = conn
        .prepare(
            "INSERT OR REPLACE INTO footnotes (
                translation_id, book_id, chapter, note_id,
                caller, text, reference, raw_json
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        )
        .map_err(|e| FreeBibleError::Generic {
            error_message: format!("prepare footnotes: {e}"),
        })?;
    let mut timing_stmt = conn
        .prepare(
            "INSERT OR REPLACE INTO audio_timings (
                translation_id, book_id, chapter, verse, start_ms, end_ms
             ) VALUES (?1,?2,?3,?4,?5,?6)",
        )
        .map_err(|e| FreeBibleError::Generic {
            error_message: format!("prepare audio_timings: {e}"),
        })?;
    let mut raw_stmt = conn
        .prepare(
            "INSERT OR REPLACE INTO chapter_raw (translation_id, book_id, chapter, raw_json)
             VALUES (?1,?2,?3,?4)",
        )
        .map_err(|e| FreeBibleError::Generic {
            error_message: format!("prepare chapter_raw: {e}"),
        })?;

    for chapter in &book.chapters {
        let merged = merge_verse_rows(&chapter.verses);
        for (number, text) in &merged {
            verse_stmt
                .execute(params![
                    translation_id,
                    book.id,
                    chapter.number,
                    number,
                    text
                ])
                .map_err(|e| FreeBibleError::Generic {
                    error_message: format!(
                        "verses insert {} {}:{}: {e}",
                        book.id, chapter.number, number
                    ),
                })?;
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
                .map_err(|e| FreeBibleError::Generic {
                    error_message: format!(
                        "chapter_items insert {} {}:{}: {e}",
                        book.id, chapter.number, item.position
                    ),
                })?;
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
                .map_err(|e| FreeBibleError::Generic {
                    error_message: format!(
                        "footnotes insert {} {}:{}: {e}",
                        book.id, chapter.number, note.note_id
                    ),
                })?;
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
                .map_err(|e| FreeBibleError::Generic {
                    error_message: format!(
                        "audio_timings insert {} {}:{}: {e}",
                        book.id, chapter.number, timing.verse
                    ),
                })?;
        }

        raw_stmt
            .execute(params![
                translation_id,
                book.id,
                chapter.number,
                chapter.raw_json
            ])
            .map_err(|e| FreeBibleError::Generic {
                error_message: format!("chapter_raw insert {} {}: {e}", book.id, chapter.number),
            })?;
    }

    Ok(())
}

pub(crate) fn mark_installed(
    conn: &Connection,
    translation_id: &str,
) -> Result<(), FreeBibleError> {
    let n = conn
        .execute(
            "UPDATE translations SET installed = 1 WHERE id = ?1",
            params![translation_id],
        )
        .map_err(|e| FreeBibleError::Generic {
            error_message: format!("mark installed: {e}"),
        })?;
    if n == 0 {
        return Err(FreeBibleError::Generic {
            error_message: format!(
                "translation {translation_id} not in catalog — call refresh() first"
            ),
        });
    }
    Ok(())
}

pub(crate) fn uninstall_translation(
    conn: &Connection,
    translation_id: &str,
) -> Result<(), FreeBibleError> {
    wipe_translation(conn, translation_id)?;
    let n = conn
        .execute(
            "UPDATE translations SET installed = 0 WHERE id = ?1",
            params![translation_id],
        )
        .map_err(|e| FreeBibleError::Generic {
            error_message: format!("clear installed flag: {e}"),
        })?;
    if n == 0 {
        return Err(FreeBibleError::Generic {
            error_message: format!("translation {translation_id} not in catalog"),
        });
    }
    info!("uninstalled {}", translation_id);
    Ok(())
}

// ── Commentary install helpers ────────────────────────────────
//
// A commentary chapter is much heavier than a translation chapter:
// Adam Clarke writes hundreds to thousands of words per verse, so
// a single chapter can be tens of KB. `persist_commentary_book`
// (whole-book transaction) took minutes on Genesis and froze the
// UI; `persist_commentary_chapter` is the per-chapter replacement.
// Network fetches are parallelized in `fetch_commentary_book_chapters`
// so the wall-clock cost is (chapters / WORKERS) × latency, not
// chapters × latency.

pub(crate) fn begin_commentary_install(
    conn: &Connection,
    commentary_id: &str,
) -> Result<(), FreeBibleError> {
    conn.execute(
        "DELETE FROM commentary_chapters WHERE commentary_id = ?1",
        params![commentary_id],
    )
    .map_err(|e| FreeBibleError::Generic {
        error_message: format!("wipe commentary_chapters: {e}"),
    })?;
    conn.execute(
        "DELETE FROM commentary_books WHERE commentary_id = ?1",
        params![commentary_id],
    )
    .map_err(|e| FreeBibleError::Generic {
        error_message: format!("wipe commentary_books: {e}"),
    })?;
    conn.execute(
        "UPDATE commentaries SET installed = 0 WHERE id = ?1",
        params![commentary_id],
    )
    .map_err(|e| FreeBibleError::Generic {
        error_message: format!("clear commentary installed flag: {e}"),
    })?;
    Ok(())
}

/// Persist the `commentary_books` row for one book, empty of chapters.
/// Called once per book, before the per-chapter loop. One writer job.
pub(crate) fn begin_commentary_book(
    conn: &Connection,
    commentary_id: &str,
    book: &FreeBibleBook,
) -> Result<(), FreeBibleError> {
    conn.execute(
        "INSERT OR REPLACE INTO commentary_books
            (commentary_id, id, name, number_of_chapters, book_order)
         VALUES (?1,?2,?3,?4,?5)",
        params![
            commentary_id,
            book.id,
            book.name,
            book.number_of_chapters,
            book.book_order,
        ],
    )
    .map_err(|e| FreeBibleError::Generic {
        error_message: format!("commentary_books insert ({}): {e}", book.id),
    })?;
    Ok(())
}

/// Persist one commentary chapter. One writer job per chapter so the
/// transaction stays small (tens of KB, not hundreds of MB) and the
/// caller can report progress between commits.
pub(crate) fn persist_commentary_chapter(
    conn: &Connection,
    commentary_id: &str,
    book_id: &str,
    chapter: &FreeBibleCommentaryChapter,
) -> Result<(), FreeBibleError> {
    let mut stmt = conn
        .prepare(
            "INSERT OR REPLACE INTO commentary_chapters
                (commentary_id, book_id, chapter, verse_number, position, text, raw_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
        )
        .map_err(|e| FreeBibleError::Generic {
            error_message: format!("prepare commentary_chapters: {e}"),
        })?;

    for (pos, v) in chapter.content.iter().enumerate() {
        stmt.execute(params![
            commentary_id,
            book_id,
            chapter.chapter_number,
            v.number,
            pos as i32,
            v.text,
            "{}",
        ])
        .map_err(|e| FreeBibleError::Generic {
            error_message: format!(
                "commentary_chapters insert {} {}:{}: {e}",
                book_id, chapter.chapter_number, v.number
            ),
        })?;
    }
    Ok(())
}

pub(crate) fn mark_commentary_installed(
    conn: &Connection,
    commentary_id: &str,
) -> Result<(), FreeBibleError> {
    let n = conn
        .execute(
            "UPDATE commentaries SET installed = 1 WHERE id = ?1",
            params![commentary_id],
        )
        .map_err(|e| FreeBibleError::Generic {
            error_message: format!("mark commentary installed: {e}"),
        })?;
    if n == 0 {
        return Err(FreeBibleError::Generic {
            error_message: format!(
                "commentary {commentary_id} not in catalog — call refresh() first"
            ),
        });
    }
    Ok(())
}

pub(crate) fn uninstall_commentary(
    conn: &Connection,
    commentary_id: &str,
) -> Result<(), FreeBibleError> {
    begin_commentary_install(conn, commentary_id)?;
    info!("uninstalled commentary {}", commentary_id);
    Ok(())
}

// ── Commentary install: parallel network side ─────────────────

/// Fetch every chapter of one book in parallel, `WORKERS` at a time.
///
/// Some commentaries skip chapters (or whole books); a 404 for one
/// chapter is logged and dropped, not fatal. Returns the fetched
/// chapters in arrival order — the writer persists each one in its
/// own job, so order does not matter for correctness, only for
/// progress reporting, and the caller counts what it gets back.
pub(crate) fn fetch_commentary_book_chapters(
    client: &BibleClient,
    commentary_id: &str,
    book: &FreeBibleBook,
    cancel: &Arc<AtomicBool>,
) -> Vec<FreeBibleCommentaryChapter> {
    let chapter_numbers: Vec<u32> = (1..=book.number_of_chapters)
        .map(|n| n as u32)
        .collect();

    let queue = Arc::new(Mutex::new(chapter_numbers));
    let (tx, rx) =
        mpsc::sync_channel::<FreeBibleCommentaryChapter>(CHANNEL_CAPACITY);

    let commentary_owned = commentary_id.to_string();
    let book_id = book.id.clone();
    let book_number_of_chapters = book.number_of_chapters;

    let mut handles = Vec::with_capacity(WORKERS);
    for _ in 0..WORKERS {
        let queue = queue.clone();
        let tx = tx.clone();
        // Cheap: shares the connection pool. Every book of the
        // install reuses the same 8 TCP/TLS connections.
        let client = client.clone();
        let cancel = cancel.clone();
        let commentary_id = commentary_owned.clone();
        let book_id = book_id.clone();

        handles.push(thread::spawn(move || loop {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let next = {
                let mut q = queue.lock().unwrap_or_else(|p| p.into_inner());
                q.pop()
            };
            let Some(ch_num) = next else { return };

            match client.commentary_chapter(&commentary_id, &book_id, ch_num) {
                Ok(ch) => {
                    if tx.send(ch).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    warn!(
                        "commentary {} {}:{} skipped: {}",
                        commentary_id, book_id, ch_num, e
                    );
                }
            }
        }));
    }
    drop(tx);

    let mut out = Vec::with_capacity(book_number_of_chapters as usize);
    while let Ok(ch) = rx.recv() {
        out.push(ch);
    }

    for h in handles {
        let _ = h.join();
    }

    out
}

// ── Entity install: network side ─────────────────────────────

/// Number of entities in the dataset for this kind. One network call.
pub(crate) fn count_entity_set(
    client: &BibleClient,
    dataset_id: &str,
    kind: EntityKind,
) -> Result<i32, FreeBibleError> {
    let summaries = match kind {
        EntityKind::Person => client.dataset_people_list(dataset_id)?,
        EntityKind::Event => client.dataset_events_list(dataset_id)?,
        EntityKind::Place => client.dataset_places_list(dataset_id)?,
        EntityKind::Group => client.dataset_groups_list(dataset_id)?,
    };
    Ok(summaries.len() as i32)
}

pub(crate) fn fetch_entity_set(
    client: &BibleClient,
    dataset_id: &str,
    kind: EntityKind,
    mut on_progress: impl FnMut(i32),
) -> Result<Vec<serde_json::Value>, FreeBibleError> {
    let summaries = match kind {
        EntityKind::Person => client.dataset_people_list(dataset_id)?,
        EntityKind::Event => client.dataset_events_list(dataset_id)?,
        EntityKind::Place => client.dataset_places_list(dataset_id)?,
        EntityKind::Group => client.dataset_groups_list(dataset_id)?,
    };
    let total = summaries.len() as i32;
    if total == 0 {
        return Err(FreeBibleError::Generic {
            error_message: format!("no {} found in dataset {}", kind.as_str(), dataset_id),
        });
    }

    let tasks: Vec<String> = summaries.iter().map(|s| s.id.clone()).collect();
    let queue = Arc::new(Mutex::new(tasks));
    let shared_client = Arc::new(BibleClient::new());
    let (tx, rx) = mpsc::sync_channel::<serde_json::Value>(CHANNEL_CAPACITY);

    let mut handles = Vec::with_capacity(WORKERS);
    for _ in 0..WORKERS {
        let queue = queue.clone();
        let tx = tx.clone();
        let client = shared_client.clone();
        let dataset = dataset_id.to_string();

        handles.push(thread::spawn(move || {
            loop {
                let next = {
                    let mut q = queue.lock().unwrap_or_else(|p| p.into_inner());
                    q.pop()
                };
                let Some(id) = next else { return };

                let result = match kind {
                    EntityKind::Person => client.dataset_person(&dataset, &id),
                    EntityKind::Event => client.dataset_event(&dataset, &id),
                    EntityKind::Place => client.dataset_place(&dataset, &id),
                    EntityKind::Group => client.dataset_group(&dataset, &id),
                };

                match result {
                    Ok(v) => {
                        if tx.send(v).is_err() {
                            return;
                        }
                    }
                    Err(e) => error!("fetch {} {} failed: {}", kind.as_str(), id, e),
                }
            }
        }));
    }
    drop(tx);

    let mut out = Vec::with_capacity(total as usize);
    let mut done = 0i32;
    while let Ok(v) = rx.recv() {
        out.push(v);
        done += 1;
        on_progress(done);
    }

    for h in handles {
        let _ = h.join();
    }

    Ok(out)
}

// ── Entity install: write side ────────────────────────────────

pub(crate) fn begin_entity_install(
    conn: &Connection,
    dataset_id: &str,
    kind: EntityKind,
) -> Result<(), FreeBibleError> {
    let table = match kind {
        EntityKind::Person => "people",
        EntityKind::Event => "events",
        EntityKind::Place => "places",
        EntityKind::Group => "people_groups",
    };
    conn.execute(
        &format!("DELETE FROM {table} WHERE dataset_id = ?1"),
        params![dataset_id],
    )
    .map_err(|e| FreeBibleError::Generic {
        error_message: format!("wipe {table}: {e}"),
    })?;
    conn.execute(
        "DELETE FROM entity_refs WHERE dataset_id = ?1 AND source_kind = ?2",
        params![dataset_id, kind.as_str()],
    )
    .map_err(|e| FreeBibleError::Generic {
        error_message: format!("wipe entity_refs: {e}"),
    })?;
    conn.execute(
        "DELETE FROM entity_verse_refs WHERE dataset_id = ?1 AND source_kind = ?2",
        params![dataset_id, kind.as_str()],
    )
    .map_err(|e| FreeBibleError::Generic {
        error_message: format!("wipe entity_verse_refs: {e}"),
    })?;
    Ok(())
}

pub(crate) fn persist_entity_batch(
    conn: &Connection,
    dataset_id: &str,
    kind: EntityKind,
    batch: &[serde_json::Value],
) -> Result<(), FreeBibleError> {
    let object_key = match kind {
        EntityKind::Person => "person",
        EntityKind::Event => "event",
        EntityKind::Place => "place",
        EntityKind::Group => "group",
    };

    for response in batch {
        let Some(obj) = response.get(object_key) else {
            warn!(
                "{} response missing `{}` field — skipping",
                kind.as_str(),
                object_key
            );
            continue;
        };
        let raw = serde_json::to_string(response).unwrap_or_default();

        let stored_id = match kind {
            EntityKind::Person => write_person(conn, dataset_id, obj, &raw)?,
            EntityKind::Event => write_event(conn, dataset_id, obj, &raw)?,
            EntityKind::Place => write_place(conn, dataset_id, obj, &raw)?,
            EntityKind::Group => write_group(conn, dataset_id, obj, &raw)?,
        };

        let refs_table = match kind {
            EntityKind::Person => PERSON_REFS,
            EntityKind::Event => EVENT_REFS,
            EntityKind::Place => PLACE_REFS,
            EntityKind::Group => GROUP_REFS,
        };
        write_entity_refs(conn, dataset_id, kind.as_str(), &stored_id, obj, refs_table)?;
        write_verse_refs(conn, dataset_id, kind.as_str(), &stored_id, obj)?;
    }

    Ok(())
}

pub(crate) fn mark_entity_installed(
    conn: &Connection,
    dataset_id: &str,
    kind: EntityKind,
    fetched: i32,
    total: i32,
) -> Result<(), FreeBibleError> {
    conn.execute(
        "INSERT OR REPLACE INTO entity_install_state
         (dataset_id, kind, installed, fetched_count, total_count)
         VALUES (?1, ?2, 1, ?3, ?4)",
        params![dataset_id, kind.as_str(), fetched, total],
    )
    .map_err(|e| FreeBibleError::Generic {
        error_message: format!("entity_install_state: {e}"),
    })?;
    Ok(())
}

// ── Per-entity writers (Connection, not Transaction) ─────────

fn write_person(
    conn: &Connection,
    dataset_id: &str,
    person: &serde_json::Value,
    raw_json: &str,
) -> Result<String, FreeBibleError> {
    let id = person
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let name = person
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let also_called = person
        .get("alsoCalled")
        .and_then(|v| v.as_array())
        .map(|arr| {
            serde_json::to_string(&arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
                .unwrap_or_default()
        });
    let is_proper_name = person
        .get("isProperName")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let gender = person.get("gender").and_then(|v| v.as_str());
    let birth_year = person.get("birthYear").and_then(|v| v.as_i64());
    let death_year = person.get("deathYear").and_then(|v| v.as_i64());
    let min_year = person.get("minYear").and_then(|v| v.as_i64());
    let max_year = person.get("maxYear").and_then(|v| v.as_i64());
    let description = join_description(
        person
            .get("description")
            .unwrap_or(&serde_json::Value::Null),
    );

    conn.execute(
        "INSERT OR REPLACE INTO people (
            dataset_id, id, name, also_called, is_proper_name, gender,
            birth_year, death_year, min_year, max_year, description, raw_json
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            dataset_id,
            id,
            name,
            also_called,
            is_proper_name as i32,
            gender,
            birth_year,
            death_year,
            min_year,
            max_year,
            description,
            raw_json,
        ],
    )
    .map_err(|e| FreeBibleError::Generic {
        error_message: format!("people insert: {e}"),
    })?;

    Ok(id)
}

fn write_event(
    conn: &Connection,
    dataset_id: &str,
    event: &serde_json::Value,
    raw_json: &str,
) -> Result<String, FreeBibleError> {
    let id = event
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let name = event
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let start_date = event.get("startDate").and_then(|v| v.as_str());
    let end_date = event.get("endDate").and_then(|v| v.as_str());
    let duration = event.get("duration").and_then(|v| v.as_str());
    let description =
        join_description(event.get("description").unwrap_or(&serde_json::Value::Null));

    conn.execute(
        "INSERT OR REPLACE INTO events (
            dataset_id, id, name, start_date, end_date, duration, description, raw_json
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            dataset_id,
            id,
            name,
            start_date,
            end_date,
            duration,
            description,
            raw_json
        ],
    )
    .map_err(|e| FreeBibleError::Generic {
        error_message: format!("events insert: {e}"),
    })?;

    Ok(id)
}

fn write_place(
    conn: &Connection,
    dataset_id: &str,
    place: &serde_json::Value,
    raw_json: &str,
) -> Result<String, FreeBibleError> {
    let id = place
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let name = place
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let kjv_name = place.get("kjvName").and_then(|v| v.as_str());
    let esv_name = place.get("esvName").and_then(|v| v.as_str());
    let feature_type = place.get("featureType").and_then(|v| v.as_str());
    let latitude = place.get("latitude").and_then(|v| v.as_f64());
    let longitude = place.get("longitude").and_then(|v| v.as_f64());
    let description =
        join_description(place.get("description").unwrap_or(&serde_json::Value::Null));

    conn.execute(
        "INSERT OR REPLACE INTO places (
            dataset_id, id, name, kjv_name, esv_name, feature_type,
            latitude, longitude, description, raw_json
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            dataset_id,
            id,
            name,
            kjv_name,
            esv_name,
            feature_type,
            latitude,
            longitude,
            description,
            raw_json,
        ],
    )
    .map_err(|e| FreeBibleError::Generic {
        error_message: format!("places insert: {e}"),
    })?;

    Ok(id)
}

fn write_group(
    conn: &Connection,
    dataset_id: &str,
    group: &serde_json::Value,
    raw_json: &str,
) -> Result<String, FreeBibleError> {
    let id = group
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let name = group
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let description =
        join_description(group.get("description").unwrap_or(&serde_json::Value::Null));

    conn.execute(
        "INSERT OR REPLACE INTO people_groups (
            dataset_id, id, name, description, raw_json
         ) VALUES (?1,?2,?3,?4,?5)",
        params![dataset_id, id, name, description, raw_json],
    )
    .map_err(|e| FreeBibleError::Generic {
        error_message: format!("people_groups insert: {e}"),
    })?;

    Ok(id)
}

// ── Graph writers ────────────────────────────────────────────

const PERSON_REFS: &[(&str, &str, &str)] = &[
    ("birthPlace", "birth_place", "place"),
    ("deathPlace", "death_place", "place"),
    ("father", "father", "person"),
    ("mother", "mother", "person"),
    ("partners", "partner", "person"),
    ("children", "child", "person"),
    ("siblings", "sibling", "person"),
    (
        "halfSiblingsSameFather",
        "half_sibling_same_father",
        "person",
    ),
    (
        "halfSiblingsSameMother",
        "half_sibling_same_mother",
        "person",
    ),
    ("memberOf", "member_of", "group"),
    ("events", "participated_in", "event"),
];

const EVENT_REFS: &[(&str, &str, &str)] = &[
    ("participants", "participant", "person"),
    ("locations", "location", "place"),
    ("predecessor", "predecessor", "event"),
    ("successor", "successor", "event"),
    ("partOf", "part_of", "event"),
];

const PLACE_REFS: &[(&str, &str, &str)] = &[("events", "event_here", "event")];

const GROUP_REFS: &[(&str, &str, &str)] = &[
    ("members", "member", "person"),
    ("leaders", "leader", "person"),
    ("leader", "leader", "person"),
    ("locatedAt", "located_at", "place"),
];

fn join_description(value: &serde_json::Value) -> Option<String> {
    let arr = value.as_array()?;
    let parts: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

fn write_entity_refs(
    conn: &Connection,
    dataset_id: &str,
    source_kind: &str,
    source_id: &str,
    obj: &serde_json::Value,
    table: &[(&str, &str, &str)],
) -> Result<(), FreeBibleError> {
    for (field, relation, default_kind) in table {
        let Some(value) = obj.get(*field) else {
            continue;
        };

        let items: Vec<&serde_json::Value> = match value {
            serde_json::Value::Array(arr) => arr.iter().collect(),
            serde_json::Value::Object(_) => vec![value],
            _ => continue,
        };

        for item in items {
            let Some(target_id) = item.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let target_name = item.get("name").and_then(|v| v.as_str());
            let target_kind = item
                .get("type")
                .and_then(|v| v.as_str())
                .map(normalize_kind)
                .unwrap_or(*default_kind);

            conn.execute(
                "INSERT OR REPLACE INTO entity_refs
                 (dataset_id, source_kind, source_id, relation,
                  target_kind, target_id, target_name)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    dataset_id,
                    source_kind,
                    source_id,
                    relation,
                    target_kind,
                    target_id,
                    target_name,
                ],
            )
            .map_err(|e| FreeBibleError::Generic {
                error_message: format!("entity_refs insert: {e}"),
            })?;
        }
    }
    Ok(())
}

fn write_verse_refs(
    conn: &Connection,
    dataset_id: &str,
    source_kind: &str,
    source_id: &str,
    obj: &serde_json::Value,
) -> Result<(), FreeBibleError> {
    let Some(refs) = obj.get("references").and_then(|v| v.as_array()) else {
        return Ok(());
    };

    for r in refs {
        let Some(book) = r.get("book").and_then(|v| v.as_str()) else {
            continue;
        };
        let chapter = r.get("chapter").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let verse = r.get("verse").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
        let end_verse = r.get("endVerse").and_then(|v| v.as_i64()).map(|x| x as i32);

        conn.execute(
            "INSERT OR REPLACE INTO entity_verse_refs
             (dataset_id, source_kind, source_id, book, chapter, verse, end_verse)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                dataset_id,
                source_kind,
                source_id,
                book,
                chapter,
                verse,
                end_verse
            ],
        )
        .map_err(|e| FreeBibleError::Generic {
            error_message: format!("entity_verse_refs insert: {e}"),
        })?;
    }
    Ok(())
}
