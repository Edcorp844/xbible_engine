use rusqlite::{Connection, params};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::engines::freebible_engine::client::{BibleClient, FreeBibleChapter, FreeBibleComplete};
use crate::engines::freebible_engine::error::FreeBibleError;
use crate::engines::freebible_engine::utils::InstallProgress;

const WORKERS: usize = 8;
const CHANNEL_CAPACITY: usize = 16;

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

fn write_person(
    tx: &rusqlite::Transaction<'_>,
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
    let birth_year = person.get("birthYear").and_then(|v| v.as_i64()) as Option<i64>;
    let death_year = person.get("deathYear").and_then(|v| v.as_i64()) as Option<i64>;
    let min_year = person.get("minYear").and_then(|v| v.as_i64()) as Option<i64>;
    let max_year = person.get("maxYear").and_then(|v| v.as_i64()) as Option<i64>;
    let description = join_description(
        person
            .get("description")
            .unwrap_or(&serde_json::Value::Null),
    );

    tx.execute(
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
    tx: &rusqlite::Transaction<'_>,
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

    tx.execute(
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
    tx: &rusqlite::Transaction<'_>,
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

    tx.execute(
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
    tx: &rusqlite::Transaction<'_>,
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

    tx.execute(
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

/// (json_field, relation_name, default_target_kind_if_type_missing)
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
    tx: &rusqlite::Transaction<'_>,
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

        // The API returns either a single object or an array of objects.
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

            tx.execute(
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
    tx: &rusqlite::Transaction<'_>,
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

        tx.execute(
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

pub fn install_entity_set(
    client: &BibleClient,
    conn: &mut Connection,
    dataset_id: &str,
    kind: EntityKind,
    mut progress: impl FnMut(InstallProgress),
    cancel: Arc<AtomicBool>,
) -> Result<String, FreeBibleError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(FreeBibleError::Generic {
            error_message: "cancelled".into(),
        });
    }

    // ── Phase 1: fetch summary list (one request) ────────────────
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

    progress(InstallProgress {
        fraction: 0.0,
        chapters_done: 0,
        chapters_total: total,
        current_book_name: String::new(),
        current_book_id: String::new(),
        current_chapter: 0,
        current_chapter_verses: 0,
        verses_done: 0,
        message: format!("Fetching {} {}…", total, kind.as_str()),
    });

    // ── Phase 2: parallel detail fetch ───────────────────────────
    let tasks: Vec<String> = summaries.iter().map(|s| s.id.clone()).collect();
    let queue = Arc::new(Mutex::new(tasks));
    let shared_client = Arc::new(BibleClient::new());
    let (tx, rx) = mpsc::sync_channel::<(String, serde_json::Value)>(CHANNEL_CAPACITY);

    let mut handles = Vec::with_capacity(WORKERS);
    for _ in 0..WORKERS {
        let queue = queue.clone();
        let tx = tx.clone();
        let client = shared_client.clone();
        let dataset_id = dataset_id.to_string();
        let cancel = cancel.clone();
        let kind = kind;

        handles.push(thread::spawn(move || {
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return;
                }
                let next = {
                    let mut q = queue.lock().unwrap();
                    q.pop()
                };
                let Some(id) = next else { return };

                let result = match kind {
                    EntityKind::Person => client.dataset_person(&dataset_id, &id),
                    EntityKind::Event => client.dataset_event(&dataset_id, &id),
                    EntityKind::Place => client.dataset_place(&dataset_id, &id),
                    EntityKind::Group => client.dataset_group(&dataset_id, &id),
                };

                match result {
                    Ok(v) => {
                        if tx.send((id, v)).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        error!("fetch {} {} failed: {}", kind.as_str(), id, e);
                    }
                }
            }
        }));
    }
    drop(tx);

    // ── Phase 3: consumer — write each detail into the DB ────────
    let result = (|| -> Result<(), FreeBibleError> {
        let db_tx = conn.transaction().map_err(|e| FreeBibleError::Generic {
            error_message: format!("begin transaction: {e}"),
        })?;

        // Wipe any previous install of this kind so reinstall is idempotent.
        let table = match kind {
            EntityKind::Person => "people",
            EntityKind::Event => "events",
            EntityKind::Place => "places",
            EntityKind::Group => "people_groups",
        };
        db_tx
            .execute(
                &format!("DELETE FROM {table} WHERE dataset_id = ?1"),
                params![dataset_id],
            )
            .map_err(|e| FreeBibleError::Generic {
                error_message: format!("wipe {table}: {e}"),
            })?;
        db_tx
            .execute(
                "DELETE FROM entity_refs WHERE dataset_id = ?1 AND source_kind = ?2",
                params![dataset_id, kind.as_str()],
            )
            .map_err(|e| FreeBibleError::Generic {
                error_message: format!("wipe entity_refs: {e}"),
            })?;
        db_tx
            .execute(
                "DELETE FROM entity_verse_refs WHERE dataset_id = ?1 AND source_kind = ?2",
                params![dataset_id, kind.as_str()],
            )
            .map_err(|e| FreeBibleError::Generic {
                error_message: format!("wipe entity_verse_refs: {e}"),
            })?;

        let mut done = 0i32;

        while let Ok((requested_id, response)) = rx.recv() {
            if cancel.load(Ordering::Relaxed) {
                return Err(FreeBibleError::Generic {
                    error_message: "cancelled".into(),
                });
            }

            // The detail response nests the object under a kind-specific key:
            //   { "dataset": {...}, "person": {...}, "thisPersonApiLink": "..." }
            let object_key = match kind {
                EntityKind::Person => "person",
                EntityKind::Event => "event",
                EntityKind::Place => "place",
                EntityKind::Group => "group",
            };
            let Some(obj) = response.get(object_key) else {
                warn!(
                    "{} response for {} missing `{}` field",
                    kind.as_str(),
                    requested_id,
                    object_key
                );
                continue;
            };

            let raw = serde_json::to_string(&response).unwrap_or_default();

            let stored_id = match kind {
                EntityKind::Person => write_person(&db_tx, dataset_id, obj, &raw)?,
                EntityKind::Event => write_event(&db_tx, dataset_id, obj, &raw)?,
                EntityKind::Place => write_place(&db_tx, dataset_id, obj, &raw)?,
                EntityKind::Group => write_group(&db_tx, dataset_id, obj, &raw)?,
            };

            // Graph edges
            let refs_table = match kind {
                EntityKind::Person => PERSON_REFS,
                EntityKind::Event => EVENT_REFS,
                EntityKind::Place => PLACE_REFS,
                EntityKind::Group => GROUP_REFS,
            };
            write_entity_refs(
                &db_tx,
                dataset_id,
                kind.as_str(),
                &stored_id,
                obj,
                refs_table,
            )?;
            write_verse_refs(&db_tx, dataset_id, kind.as_str(), &stored_id, obj)?;

            done += 1;

            progress(InstallProgress {
                fraction: 0.8 + (done as f64 / total as f64) * 0.2,
                chapters_done: done,
                chapters_total: total,
                current_book_name: obj
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                current_book_id: stored_id.clone(),
                current_chapter: 0,
                current_chapter_verses: 0,
                verses_done: done,
                message: format!(
                    "{}: {} ({}/{})",
                    kind.as_str(),
                    obj.get("name").and_then(|v| v.as_str()).unwrap_or("?"),
                    done,
                    total,
                ),
            });
        }

        // Mark installed
        db_tx
            .execute(
                "INSERT OR REPLACE INTO entity_install_state
                 (dataset_id, kind, installed, fetched_count, total_count)
                 VALUES (?1, ?2, 1, ?3, ?4)",
                params![dataset_id, kind.as_str(), done, total],
            )
            .map_err(|e| FreeBibleError::Generic {
                error_message: format!("entity_install_state: {e}"),
            })?;

        db_tx.commit().map_err(|e| FreeBibleError::Generic {
            error_message: format!("commit: {e}"),
        })?;
        Ok(())
    })();

    for h in handles {
        let _ = h.join();
    }

    result.map(|_| format!("{}:{}", dataset_id, kind.as_str()))
}

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
        .map_err(|e| FreeBibleError::Generic {
            error_message: e.to_string(),
        })?;
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
                        ch.number,
                        ch.verses.len(),
                        ch.items.len(),
                        ch.footnotes.len(),
                        ch.audio_timings.len(),
                        ch.raw_json.len()
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

    let tx = conn.transaction().map_err(|e| FreeBibleError::Generic {
        error_message: e.to_string(),
    })?;
    wipe_translation(&tx, translation_id)?;

    // Books + per-book metadata
    {
        let mut book_stmt = tx
            .prepare(
                "INSERT INTO books (translation_id, id, name, number_of_chapters)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .map_err(|e| FreeBibleError::Generic {
                error_message: e.to_string(),
            })?;
        let mut meta_stmt = tx
            .prepare(
                "INSERT INTO book_metadata (
                    translation_id, book_id, common_name, title,
                    book_order, audio_links_json, raw_json
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            )
            .map_err(|e| FreeBibleError::Generic {
                error_message: e.to_string(),
            })?;

        for b in &complete.books {
            book_stmt
                .execute(params![translation_id, b.id, b.name, b.number_of_chapters])
                .map_err(|e| FreeBibleError::Generic {
                    error_message: e.to_string(),
                })?;
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
                .map_err(|e| FreeBibleError::Generic {
                    error_message: e.to_string(),
                })?;
        }
    }

    // Chapter-scoped prepared statements
    let mut verse_stmt = tx
        .prepare(
            "INSERT INTO verses (translation_id, book_id, chapter, verse, text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .map_err(|e| FreeBibleError::Generic {
            error_message: e.to_string(),
        })?;
    let mut item_stmt = tx
        .prepare(
            "INSERT INTO chapter_items (
                translation_id, book_id, chapter, position, item_type,
                verse_number, text, poem, words_of_jesus, footnote_ids, raw_json
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        )
        .map_err(|e| FreeBibleError::Generic {
            error_message: e.to_string(),
        })?;
    let mut footnote_stmt = tx
        .prepare(
            "INSERT INTO footnotes (
                translation_id, book_id, chapter, note_id,
                caller, text, reference, raw_json
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        )
        .map_err(|e| FreeBibleError::Generic {
            error_message: e.to_string(),
        })?;
    let mut timing_stmt = tx
        .prepare(
            "INSERT INTO audio_timings (
                translation_id, book_id, chapter, verse, start_ms, end_ms
             ) VALUES (?1,?2,?3,?4,?5,?6)",
        )
        .map_err(|e| FreeBibleError::Generic {
            error_message: e.to_string(),
        })?;
    let mut raw_stmt = tx
        .prepare(
            "INSERT INTO chapter_raw (translation_id, book_id, chapter, raw_json)
             VALUES (?1,?2,?3,?4)",
        )
        .map_err(|e| FreeBibleError::Generic {
            error_message: e.to_string(),
        })?;

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
                    .map_err(|e| FreeBibleError::Generic {
                        error_message: e.to_string(),
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
                        error_message: e.to_string(),
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
                        error_message: e.to_string(),
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
                        error_message: e.to_string(),
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
                    error_message: e.to_string(),
                })?;

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
    .map_err(|e| FreeBibleError::Generic {
        error_message: e.to_string(),
    })?;

    tx.commit().map_err(|e| FreeBibleError::Generic {
        error_message: e.to_string(),
    })?;
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
    let (tx, rx) = mpsc::sync_channel::<(String, String, i32, FreeBibleChapter)>(CHANNEL_CAPACITY);

    let mut handles = Vec::with_capacity(WORKERS);
    for _ in 0..WORKERS {
        let queue = queue.clone();
        let tx = tx.clone();
        let client = shared_client.clone();
        let tid = translation_id.to_string();
        let cancel = cancel.clone();

        handles.push(thread::spawn(move || {
            loop {
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
            }
        }));
    }
    drop(tx);

    let result = (|| -> Result<(), FreeBibleError> {
        let db_tx = conn.transaction().map_err(|e| FreeBibleError::Generic {
            error_message: e.to_string(),
        })?;
        wipe_translation(&db_tx, translation_id)?;

        {
            let mut stmt = db_tx
                .prepare(
                    "INSERT INTO books (translation_id, id, name, number_of_chapters)
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|e| FreeBibleError::Generic {
                    error_message: e.to_string(),
                })?;
            for b in &books {
                stmt.execute(params![translation_id, b.id, b.name, b.number_of_chapters])
                    .map_err(|e| FreeBibleError::Generic {
                        error_message: e.to_string(),
                    })?;
            }
        }

        let mut verse_stmt = db_tx
            .prepare(
                "INSERT INTO verses (translation_id, book_id, chapter, verse, text)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .map_err(|e| FreeBibleError::Generic {
                error_message: e.to_string(),
            })?;

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
                    .map_err(|e| FreeBibleError::Generic {
                        error_message: e.to_string(),
                    })?;
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
            .map_err(|e| FreeBibleError::Generic {
                error_message: e.to_string(),
            })?;

        db_tx.commit().map_err(|e| FreeBibleError::Generic {
            error_message: e.to_string(),
        })?;
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
    let tx = conn.transaction().map_err(|e| FreeBibleError::Generic {
        error_message: e.to_string(),
    })?;
    wipe_translation(&tx, translation_id)?;
    tx.execute(
        "UPDATE translations SET installed = 0 WHERE id = ?1",
        params![translation_id],
    )
    .map_err(|e| FreeBibleError::Generic {
        error_message: e.to_string(),
    })?;
    tx.commit().map_err(|e| FreeBibleError::Generic {
        error_message: e.to_string(),
    })?;
    info!("uninstalled {}", translation_id);
    Ok(translation_id.to_string())
}
