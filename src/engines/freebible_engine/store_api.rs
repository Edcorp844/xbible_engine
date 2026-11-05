use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use log::{error, info};

use crate::db;
use crate::engines::freebible_engine::client::{
    BibleClient, FreeBibleBook, FreeBibleChapterItem, FreeBibleFootnote, FreeBibleVerse,
};
use crate::engines::freebible_engine::store::{LanguageFacet, Store, StoreItem, StoreShelf};
use crate::engines::freebible_engine::utils::{
    FreeBibleError, FreeBibleTaskState, FreeBibleTaskStatus, InstallProgress,
};
use crate::engines::freebible_engine::{EntitySummary, install};

#[derive(Debug, Clone, uniffi::Record)]
pub struct EntityRef {
    pub kind: String, // "person" | "event" | "place" | "group"
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct VerseRange {
    pub book: String,
    pub chapter: i32,
    pub verse: i32,
    pub end_verse: Option<i32>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct PersonDetail {
    pub id: String,
    pub name: String,
    pub also_called: Vec<String>,
    pub gender: Option<String>,
    pub birth_year: Option<i64>,
    pub death_year: Option<i64>,
    pub min_year: Option<i64>,
    pub max_year: Option<i64>,
    pub description: Option<String>,

    // Relations
    pub father: Vec<EntityRef>,
    pub mother: Vec<EntityRef>,
    pub partners: Vec<EntityRef>,
    pub children: Vec<EntityRef>,
    pub siblings: Vec<EntityRef>,
    pub half_siblings_same_father: Vec<EntityRef>,
    pub half_siblings_same_mother: Vec<EntityRef>,
    pub member_of: Vec<EntityRef>,

    pub birth_place: Vec<EntityRef>,
    pub death_place: Vec<EntityRef>,

    // Forward: events Joseph is listed in
    pub events: Vec<EntityRef>,
    // Reverse: events that list Joseph as participant (may differ)
    pub referenced_by_events: Vec<EntityRef>,

    pub verses: Vec<VerseRange>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct EventDetail {
    pub id: String,
    pub name: String,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub duration: Option<String>,
    pub description: Option<String>,

    pub participants: Vec<EntityRef>,
    pub locations: Vec<EntityRef>,
    pub predecessor: Option<EntityRef>,
    pub successor: Option<EntityRef>,
    pub parent_event: Option<EntityRef>,
    pub sub_events: Vec<EntityRef>,

    pub verses: Vec<VerseRange>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct PlaceDetail {
    pub id: String,
    pub name: String,
    pub kjv_name: Option<String>,
    pub esv_name: Option<String>,
    pub feature_type: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub description: Option<String>,

    pub events: Vec<EntityRef>,
    pub people_born_here: Vec<EntityRef>,
    pub people_died_here: Vec<EntityRef>,
    pub people_groups_here: Vec<EntityRef>,

    pub verses: Vec<VerseRange>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct GroupDetail {
    pub id: String,
    pub name: String,
    pub description: Option<String>,

    pub members: Vec<EntityRef>,
    pub leaders: Vec<EntityRef>,
    pub located_at: Vec<EntityRef>,

    pub verses: Vec<VerseRange>,
}

// Forward edges from an entity: "give me everything X points to, filtered by relation(s)"
fn read_forward_refs(
    conn: &rusqlite::Connection,
    dataset_id: &str,
    kind: &str,
    id: &str,
    relations: &[&str],
) -> Vec<EntityRef> {
    if relations.is_empty() {
        return Vec::new();
    }
    let placeholders = relations
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 4))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT target_kind, target_id, COALESCE(target_name, target_id)
         FROM entity_refs
         WHERE dataset_id = ?1 AND source_kind = ?2 AND source_id = ?3
           AND relation IN ({placeholders})
         ORDER BY relation, target_name"
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut params: Vec<&dyn rusqlite::ToSql> = vec![&dataset_id, &kind, &id];
    for r in relations {
        params.push(r);
    }
    match stmt.query_map(&params[..], |row| {
        Ok(EntityRef {
            kind: row.get(0)?,
            id: row.get(1)?,
            name: row.get(2)?,
        })
    }) {
        Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

fn read_forward_refs_all(
    conn: &rusqlite::Connection,
    dataset_id: &str,
    kind: &str,
    id: &str,
) -> Vec<EntityRef> {
    let mut stmt = match conn.prepare(
        "SELECT target_kind, target_id, COALESCE(target_name, target_id)
         FROM entity_refs
         WHERE dataset_id = ?1 AND source_kind = ?2 AND source_id = ?3
         ORDER BY relation, target_name",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    match stmt.query_map(rusqlite::params![dataset_id, kind, id], |row| {
        Ok(EntityRef {
            kind: row.get(0)?,
            id: row.get(1)?,
            name: row.get(2)?,
        })
    }) {
        Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

// Reverse edges: "who points at X, filtered by relation(s)"
fn read_reverse_refs(
    conn: &rusqlite::Connection,
    dataset_id: &str,
    target_kind: &str,
    target_id: &str,
    relations: &[&str],
) -> Vec<EntityRef> {
    if relations.is_empty() {
        return Vec::new();
    }
    let placeholders = relations
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 4))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT source_kind, source_id,
                COALESCE((SELECT name FROM people WHERE id = source_id AND dataset_id = ?1),
                         (SELECT name FROM events WHERE id = source_id AND dataset_id = ?1),
                         (SELECT name FROM places WHERE id = source_id AND dataset_id = ?1),
                         (SELECT name FROM people_groups WHERE id = source_id AND dataset_id = ?1),
                         source_id)
         FROM entity_refs
         WHERE dataset_id = ?1 AND target_kind = ?2 AND target_id = ?3
           AND relation IN ({placeholders})
         ORDER BY relation"
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut params: Vec<&dyn rusqlite::ToSql> = vec![&dataset_id, &target_kind, &target_id];
    for r in relations {
        params.push(r);
    }
    match stmt.query_map(&params[..], |row| {
        Ok(EntityRef {
            kind: row.get(0)?,
            id: row.get(1)?,
            name: row.get(2)?,
        })
    }) {
        Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

// Verse ranges for an entity
fn read_verse_refs(
    conn: &rusqlite::Connection,
    dataset_id: &str,
    kind: &str,
    id: &str,
) -> Vec<VerseRange> {
    let mut stmt = match conn.prepare(
        "SELECT book, chapter, verse, end_verse
         FROM entity_verse_refs
         WHERE dataset_id = ?1 AND source_kind = ?2 AND source_id = ?3
         ORDER BY book, chapter, verse",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    match stmt.query_map(rusqlite::params![dataset_id, kind, id], |row| {
        Ok(VerseRange {
            book: row.get(0)?,
            chapter: row.get(1)?,
            verse: row.get(2)?,
            end_verse: row.get(3)?,
        })
    }) {
        Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

#[derive(Debug)]
struct TaskEntry {
    status: FreeBibleTaskStatus,
    cancel: Arc<AtomicBool>,
}

#[derive(Debug, uniffi::Object)]
pub struct FreeBibleStore {
    inner: RwLock<Option<Arc<Store>>>,
    client: BibleClient,
    conn: RwLock<Option<Arc<Mutex<rusqlite::Connection>>>>,
    tasks: Arc<Mutex<HashMap<String, TaskEntry>>>,
    next_id: Arc<Mutex<u64>>,
}

// ── Helpers ─────────────────────────────────────────────────────────

fn lock_or_recover<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn read_or_recover<T>(m: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_or_recover<T>(m: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    m.write().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn alloc_task_id(next_id: &Arc<Mutex<u64>>, kind: &str, target: &str) -> String {
    let mut id = lock_or_recover(next_id);
    let task = format!("{}_{}_{}", kind, target, *id);
    *id += 1;
    task
}

fn register_task(
    tasks: &Arc<Mutex<HashMap<String, TaskEntry>>>,
    task_id: &str,
    target: &str,
    message: &str,
    cancel: &Arc<AtomicBool>,
) {
    let entry = TaskEntry {
        status: FreeBibleTaskStatus {
            task_id: task_id.to_string(),
            state: FreeBibleTaskState::Queued,
            progress: InstallProgress {
                fraction: 0.0,
                chapters_done: 0,
                chapters_total: 0,
                current_book_name: String::new(),
                current_book_id: String::new(),
                current_chapter: 0,
                current_chapter_verses: 0,
                verses_done: 0,
                message: format!("{} {}", message, target),
            },
        },
        cancel: cancel.clone(),
    };
    lock_or_recover(tasks).insert(task_id.to_string(), entry);
}

fn set_state<F>(
    tasks: &Arc<Mutex<HashMap<String, TaskEntry>>>,
    task_id: &str,
    state: FreeBibleTaskState,
    mutate: F,
) where
    F: FnOnce(&mut InstallProgress),
{
    if let Some(t) = lock_or_recover(tasks).get_mut(task_id) {
        t.status.state = state;
        mutate(&mut t.status.progress);
    }
}

fn is_cancelled(err: &FreeBibleError) -> bool {
    matches!(err, FreeBibleError::Generic { error_message } if error_message == "cancelled")
}

fn format_err(err: &FreeBibleError) -> String {
    match err {
        FreeBibleError::Generic { error_message } => error_message.clone(),
    }
}

// ── UniFFI object ────────────────────────────────────────────────────

#[uniffi::export]
impl FreeBibleStore {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(None),
            client: BibleClient::new(),
            conn: RwLock::new(None),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(0)),
        })
    }

    pub fn set_db_path(&self, path: String) -> Result<(), FreeBibleError> {
        let conn = db::open(&path).map_err(|e| FreeBibleError::Generic {
            error_message: e.to_string(),
        })?;
        *write_or_recover(&self.conn) = Some(Arc::new(Mutex::new(conn)));
        info!("database opened at {}", path);
        Ok(())
    }

    // ── Catalog ──────────────────────────────────────────────────

    pub fn refresh(&self) -> Result<(), FreeBibleError> {
        // Snapshot the connection Arc, release the outer lock immediately.
        let conn_arc = read_or_recover(&self.conn).clone();

        // Fetch + persist without holding any store lock across network I/O.
        // Store::fetch takes the connection's Mutex internally — that's the
        // only lock held, and only during the write phase.
        let store = if let Some(conn_arc) = &conn_arc {
            let conn = lock_or_recover(conn_arc);
            Store::fetch(&self.client, Some(&conn))
        } else {
            Store::fetch(&self.client, None)
        }
        .map_err(|e| FreeBibleError::Generic { error_message: e })?;

        *write_or_recover(&self.inner) = Some(Arc::new(store));
        info!("catalog refreshed");
        Ok(())
    }

    pub fn home_feed(&self, language: Option<String>) -> Vec<StoreShelf> {
        read_or_recover(&self.inner)
            .as_ref()
            .map(|s| s.home_feed(language.as_deref()))
            .unwrap_or_default()
    }

    pub fn search(&self, query: String) -> Vec<StoreItem> {
        read_or_recover(&self.inner)
            .as_ref()
            .map(|s| s.search(&query))
            .unwrap_or_default()
    }

    pub fn languages(&self) -> Vec<LanguageFacet> {
        read_or_recover(&self.inner)
            .as_ref()
            .map(|s| s.languages())
            .unwrap_or_default()
    }

    pub fn item(&self, id: String, kind: String) -> Option<StoreItem> {
        read_or_recover(&self.inner)
            .as_ref()
            .and_then(|s| s.item(&id, &kind))
    }

    // ── Read path ────────────────────────────────────────────────

    pub fn get_books(&self, translation_id: String) -> Vec<FreeBibleBook> {
        let Some(conn_arc) = read_or_recover(&self.conn).clone() else {
            return Vec::new();
        };
        let conn = lock_or_recover(&conn_arc);

        let mut stmt = match conn.prepare(
            "SELECT b.id, b.name, b.number_of_chapters,
                    m.common_name, m.title, m.book_order,
                    m.audio_links_json, m.raw_json
             FROM books b
             LEFT JOIN book_metadata m
               ON m.translation_id = b.translation_id AND m.book_id = b.id
             WHERE b.translation_id = ?1
             ORDER BY COALESCE(m.book_order, 999999), b.id",
        ) {
            Ok(s) => s,
            Err(e) => {
                error!("get_books prepare failed: {}", e);
                return Vec::new();
            }
        };

        let rows = stmt.query_map(rusqlite::params![translation_id], |r| {
            Ok(FreeBibleBook {
                id: r.get(0)?,
                name: r.get(1)?,
                number_of_chapters: r.get(2)?,
                common_name: r.get(3)?,
                title: r.get(4)?,
                book_order: r.get(5)?,
                audio_links_json: r.get(6)?,
                raw_json: r.get(7)?,
            })
        });

        match rows {
            Ok(r) => r.filter_map(|x| x.ok()).collect(),
            Err(e) => {
                error!("get_books query failed: {}", e);
                Vec::new()
            }
        }
    }

    pub fn get_chapter(
        &self,
        translation_id: String,
        book_id: String,
        chapter: i32,
    ) -> Vec<FreeBibleVerse> {
        let Some(conn_arc) = read_or_recover(&self.conn).clone() else {
            return Vec::new();
        };
        let conn = lock_or_recover(&conn_arc);

        let mut stmt = match conn.prepare(
            "SELECT verse, text FROM verses
             WHERE translation_id = ?1 AND book_id = ?2 AND chapter = ?3
             ORDER BY verse",
        ) {
            Ok(s) => s,
            Err(e) => {
                error!("get_chapter prepare failed: {}", e);
                return Vec::new();
            }
        };

        let rows = stmt.query_map(rusqlite::params![translation_id, book_id, chapter], |r| {
            Ok(FreeBibleVerse {
                number: r.get(0)?,
                text: r.get(1)?,
            })
        });

        match rows {
            Ok(r) => r.filter_map(|x| x.ok()).collect(),
            Err(e) => {
                error!("get_chapter query failed: {}", e);
                Vec::new()
            }
        }
    }

    pub fn get_verse(
        &self,
        translation_id: String,
        book_id: String,
        chapter: i32,
        verse: i32,
    ) -> Option<String> {
        let conn_arc = read_or_recover(&self.conn).clone()?;
        let conn = lock_or_recover(&conn_arc);

        conn.query_row(
            "SELECT text FROM verses
             WHERE translation_id = ?1 AND book_id = ?2 AND chapter = ?3 AND verse = ?4",
            rusqlite::params![translation_id, book_id, chapter, verse],
            |r| r.get::<_, String>(0),
        )
        .ok()
    }

    pub fn get_chapter_items(
        &self,
        translation_id: String,
        book_id: String,
        chapter: i32,
    ) -> Vec<FreeBibleChapterItem> {
        let Some(conn_arc) = read_or_recover(&self.conn).clone() else {
            return Vec::new();
        };
        let conn = lock_or_recover(&conn_arc);

        let mut stmt = match conn.prepare(
            "SELECT position, item_type, verse_number, text,
                    poem, words_of_jesus, footnote_ids, raw_json
             FROM chapter_items
             WHERE translation_id = ?1 AND book_id = ?2 AND chapter = ?3
             ORDER BY position",
        ) {
            Ok(s) => s,
            Err(e) => {
                error!("get_chapter_items prepare failed: {}", e);
                return Vec::new();
            }
        };

        let rows = stmt.query_map(rusqlite::params![translation_id, book_id, chapter], |r| {
            let footnote_ids: Option<String> = r.get(6)?;
            Ok(FreeBibleChapterItem {
                position: r.get(0)?,
                item_type: r.get(1)?,
                verse_number: r.get(2)?,
                text: r.get(3)?,
                poem: r.get::<_, i32>(4)? != 0,
                words_of_jesus: r.get::<_, i32>(5)? != 0,
                footnote_ids: footnote_ids
                    .map(|s| {
                        s.split(',')
                            .filter(|x| !x.is_empty())
                            .map(|x| x.to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
                raw_json: r.get::<_, Option<String>>(7)?.unwrap_or_default(),
            })
        });

        match rows {
            Ok(r) => r.filter_map(|x| x.ok()).collect(),
            Err(e) => {
                error!("get_chapter_items query failed: {}", e);
                Vec::new()
            }
        }
    }

    pub fn get_footnotes(
        &self,
        translation_id: String,
        book_id: String,
        chapter: i32,
    ) -> Vec<FreeBibleFootnote> {
        let Some(conn_arc) = read_or_recover(&self.conn).clone() else {
            return Vec::new();
        };
        let conn = lock_or_recover(&conn_arc);

        let mut stmt = match conn.prepare(
            "SELECT note_id, caller, text, reference, raw_json
             FROM footnotes
             WHERE translation_id = ?1 AND book_id = ?2 AND chapter = ?3",
        ) {
            Ok(s) => s,
            Err(e) => {
                error!("get_footnotes prepare failed: {}", e);
                return Vec::new();
            }
        };

        let rows = stmt.query_map(rusqlite::params![translation_id, book_id, chapter], |r| {
            Ok(FreeBibleFootnote {
                note_id: r.get(0)?,
                caller: r.get(1)?,
                text: r.get(2)?,
                reference: r.get(3)?,
                raw_json: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
            })
        });

        match rows {
            Ok(r) => r.filter_map(|x| x.ok()).collect(),
            Err(e) => {
                error!("get_footnotes query failed: {}", e);
                Vec::new()
            }
        }
    }

    pub fn get_person(&self, dataset_id: String, id: String) -> Option<PersonDetail> {
        let conn_arc = read_or_recover(&self.conn).clone()?;
        let conn = lock_or_recover(&conn_arc);

        let row = conn
            .query_row(
                "SELECT id, name, also_called, gender, birth_year, death_year,
                    min_year, max_year, description
             FROM people WHERE dataset_id = ?1 AND id = ?2",
                rusqlite::params![dataset_id, id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<i64>>(4)?,
                        r.get::<_, Option<i64>>(5)?,
                        r.get::<_, Option<i64>>(6)?,
                        r.get::<_, Option<i64>>(7)?,
                        r.get::<_, Option<String>>(8)?,
                    ))
                },
            )
            .ok()?;

        let also_called: Vec<String> = row
            .2
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        Some(PersonDetail {
            id: row.0.clone(),
            name: row.1,
            also_called,
            gender: row.3,
            birth_year: row.4,
            death_year: row.5,
            min_year: row.6,
            max_year: row.7,
            description: row.8,

            father: read_forward_refs(&conn, &dataset_id, "person", &id, &["father"]),
            mother: read_forward_refs(&conn, &dataset_id, "person", &id, &["mother"]),
            partners: read_forward_refs(&conn, &dataset_id, "person", &id, &["partner"]),
            children: read_forward_refs(&conn, &dataset_id, "person", &id, &["child"]),
            siblings: read_forward_refs(&conn, &dataset_id, "person", &id, &["sibling"]),
            half_siblings_same_father: read_forward_refs(
                &conn,
                &dataset_id,
                "person",
                &id,
                &["half_sibling_same_father"],
            ),
            half_siblings_same_mother: read_forward_refs(
                &conn,
                &dataset_id,
                "person",
                &id,
                &["half_sibling_same_mother"],
            ),
            member_of: read_forward_refs(&conn, &dataset_id, "person", &id, &["member_of"]),

            birth_place: read_forward_refs(&conn, &dataset_id, "person", &id, &["birth_place"]),
            death_place: read_forward_refs(&conn, &dataset_id, "person", &id, &["death_place"]),

            events: read_forward_refs(&conn, &dataset_id, "person", &id, &["participated_in"]),
            referenced_by_events: read_reverse_refs(
                &conn,
                &dataset_id,
                "person",
                &id,
                &["participant"],
            ),

            verses: read_verse_refs(&conn, &dataset_id, "person", &id),
        })
    }

    pub fn get_event(&self, dataset_id: String, id: String) -> Option<EventDetail> {
        let conn_arc = read_or_recover(&self.conn).clone()?;
        let conn = lock_or_recover(&conn_arc);

        let row = conn
            .query_row(
                "SELECT name, start_date, end_date, duration, description
             FROM events WHERE dataset_id = ?1 AND id = ?2",
                rusqlite::params![dataset_id, id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .ok()?;

        Some(EventDetail {
            id: id.clone(),
            name: row.0,
            start_date: row.1,
            end_date: row.2,
            duration: row.3,
            description: row.4,

            participants: read_forward_refs(&conn, &dataset_id, "event", &id, &["participant"]),
            locations: read_forward_refs(&conn, &dataset_id, "event", &id, &["location"]),
            predecessor: read_forward_refs(&conn, &dataset_id, "event", &id, &["predecessor"])
                .into_iter()
                .next(),
            successor: read_forward_refs(&conn, &dataset_id, "event", &id, &["successor"])
                .into_iter()
                .next(),
            parent_event: read_forward_refs(&conn, &dataset_id, "event", &id, &["part_of"])
                .into_iter()
                .next(),
            sub_events: read_reverse_refs(&conn, &dataset_id, "event", &id, &["part_of"]),

            verses: read_verse_refs(&conn, &dataset_id, "event", &id),
        })
    }

    pub fn get_group(&self, dataset_id: String, id: String) -> Option<GroupDetail> {
        let conn_arc = read_or_recover(&self.conn).clone()?;
        let conn = lock_or_recover(&conn_arc);

        let row = conn
            .query_row(
                "SELECT name, description FROM people_groups
             WHERE dataset_id = ?1 AND id = ?2",
                rusqlite::params![dataset_id, id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .ok()?;

        Some(GroupDetail {
            id: id.clone(),
            name: row.0,
            description: row.1,

            members: read_forward_refs(&conn, &dataset_id, "group", &id, &["member"]),
            leaders: read_forward_refs(&conn, &dataset_id, "group", &id, &["leader"]),
            located_at: read_forward_refs(&conn, &dataset_id, "group", &id, &["located_at"]),

            verses: read_verse_refs(&conn, &dataset_id, "group", &id),
        })
    }

    pub fn get_place(&self, dataset_id: String, id: String) -> Option<PlaceDetail> {
        let conn_arc = read_or_recover(&self.conn).clone()?;
        let conn = lock_or_recover(&conn_arc);

        let row = conn
            .query_row(
                "SELECT name, kjv_name, esv_name, feature_type, latitude, longitude, description
             FROM places WHERE dataset_id = ?1 AND id = ?2",
                rusqlite::params![dataset_id, id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<f64>>(4)?,
                        r.get::<_, Option<f64>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .ok()?;

        Some(PlaceDetail {
            id: id.clone(),
            name: row.0,
            kjv_name: row.1,
            esv_name: row.2,
            feature_type: row.3,
            latitude: row.4,
            longitude: row.5,
            description: row.6,

            events: read_forward_refs(&conn, &dataset_id, "place", &id, &["event_here"]),
            people_born_here: read_reverse_refs(&conn, &dataset_id, "place", &id, &["birth_place"]),
            people_died_here: read_reverse_refs(&conn, &dataset_id, "place", &id, &["death_place"]),
            people_groups_here: read_reverse_refs(
                &conn,
                &dataset_id,
                "place",
                &id,
                &["located_at"],
            ),

            verses: read_verse_refs(&conn, &dataset_id, "place", &id),
        })
    }

    pub fn list_people(
        &self,
        dataset_id: String,
        offset: i32,
        limit: i32,
        query: Option<String>,
    ) -> Vec<EntitySummary> {
        let conn_arc = match read_or_recover(&self.conn).clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = lock_or_recover(&conn_arc);

        let (sql, param_list): (String, Vec<Box<dyn rusqlite::ToSql>>) = match query {
        Some(q) if !q.trim().is_empty() => (
            "SELECT id, name, is_proper_name, gender, NULL, 
                    (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='person' AND source_id=people.id)
             FROM people WHERE dataset_id=?1 AND name LIKE ?2
             ORDER BY name LIMIT ?3 OFFSET ?4".into(),
            vec![
                Box::new(dataset_id.clone()),
                Box::new(format!("%{}%", q)),
                Box::new(limit),
                Box::new(offset),
            ],
        ),
        _ => (
            "SELECT id, name, is_proper_name, gender, NULL,
                    (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='person' AND source_id=people.id)
             FROM people WHERE dataset_id=?1
             ORDER BY name LIMIT ?2 OFFSET ?3".into(),
            vec![Box::new(dataset_id.clone()), Box::new(limit), Box::new(offset)],
        ),
    };

        let params: Vec<&dyn rusqlite::ToSql> = param_list.iter().map(|b| b.as_ref()).collect();
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match stmt.query_map(&params[..], |r| {
            Ok(EntitySummary {
                id: r.get(0)?,
                name: r.get(1)?,
                is_proper_name: r.get::<_, Option<i32>>(2)?.map(|v| v != 0),
                gender: r.get(3)?,
                start_date: r.get(4)?,
                number_of_references: r.get(5)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    // ── Entity install state ─────────────────────────────────────

    pub fn is_entity_set_installed(&self, dataset_id: String, kind: String) -> bool {
        let Some(conn_arc) = read_or_recover(&self.conn).clone() else {
            return false;
        };
        let conn = lock_or_recover(&conn_arc);
        conn.query_row(
            "SELECT installed FROM entity_install_state
             WHERE dataset_id = ?1 AND kind = ?2",
            rusqlite::params![dataset_id, kind],
            |r| r.get::<_, i32>(0),
        )
        .map(|v| v == 1)
        .unwrap_or(false)
    }

    // ── Entity install (spawns background task) ──────────────────

    pub fn install_entity_set(
        &self,
        dataset_id: String,
        kind_str: String,
    ) -> Result<String, FreeBibleError> {
        let kind = match kind_str.as_str() {
            "person" => install::EntityKind::Person,
            "event" => install::EntityKind::Event,
            "place" => install::EntityKind::Place,
            "group" => install::EntityKind::Group,
            other => {
                return Err(FreeBibleError::Generic {
                    error_message: format!("unknown entity kind: {other}"),
                });
            }
        };

        let conn_arc =
            read_or_recover(&self.conn)
                .clone()
                .ok_or_else(|| FreeBibleError::Generic {
                    error_message: "call set_db_path first".into(),
                })?;

        let task_id = alloc_task_id(&self.next_id, "entity", &kind_str);
        let cancel = Arc::new(AtomicBool::new(false));

        register_task(&self.tasks, &task_id, &kind_str, "Queued", &cancel);

        let tid = task_id.clone();
        let tasks = self.tasks.clone();
        let client = BibleClient::new();
        let dataset = dataset_id.clone();
        let kind_label = kind_str.clone();

        thread::spawn(move || {
            set_state(&tasks, &tid, FreeBibleTaskState::Running, |p| {
                p.message = format!("Preparing {} install…", kind_label);
            });

            let mut conn = lock_or_recover(&conn_arc);

            let result = install::install_entity_set(
                &client,
                &mut conn,
                &dataset,
                kind,
                |p: InstallProgress| {
                    if let Some(t) = lock_or_recover(&tasks).get_mut(&tid) {
                        t.status.progress = p;
                    }
                },
                cancel.clone(),
            );

            drop(conn);

            match result {
                Ok(_) => {
                    info!("entity set {}:{} installed", dataset, kind_label);
                    set_state(&tasks, &tid, FreeBibleTaskState::Completed, |p| {
                        p.fraction = 1.0;
                        p.message = format!("{} installed", kind_label);
                    });
                }
                Err(e) if is_cancelled(&e) => {
                    set_state(&tasks, &tid, FreeBibleTaskState::Cancelled, |p| {
                        p.message = "Cancelled".into();
                    });
                }
                Err(e) => {
                    let msg = format_err(&e);
                    error!("entity set {}:{} failed: {}", dataset, kind_label, msg);
                    set_state(
                        &tasks,
                        &tid,
                        FreeBibleTaskState::Failed { error: msg.clone() },
                        |p| {
                            p.message = format!("Install failed: {msg}");
                        },
                    );
                }
            }
        });

        Ok(task_id)
    }

    // ── Entity browse: events, places, groups ────────────────────

    pub fn list_events(
        &self,
        dataset_id: String,
        offset: i32,
        limit: i32,
        query: Option<String>,
    ) -> Vec<EntitySummary> {
        let Some(conn_arc) = read_or_recover(&self.conn).clone() else {
            return Vec::new();
        };
        let conn = lock_or_recover(&conn_arc);

        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::ToSql>>) = match query {
            Some(q) if !q.trim().is_empty() => (
                "SELECT id, name, NULL, NULL, start_date,
                        (SELECT COUNT(*) FROM entity_refs
                         WHERE dataset_id=?1 AND source_kind='event' AND source_id=events.id)
                 FROM events WHERE dataset_id=?1 AND name LIKE ?2
                 ORDER BY name LIMIT ?3 OFFSET ?4"
                    .into(),
                vec![
                    Box::new(dataset_id.clone()),
                    Box::new(format!("%{}%", q)),
                    Box::new(limit),
                    Box::new(offset),
                ],
            ),
            _ => (
                "SELECT id, name, NULL, NULL, start_date,
                        (SELECT COUNT(*) FROM entity_refs
                         WHERE dataset_id=?1 AND source_kind='event' AND source_id=events.id)
                 FROM events WHERE dataset_id=?1
                 ORDER BY name LIMIT ?2 OFFSET ?3"
                    .into(),
                vec![
                    Box::new(dataset_id.clone()),
                    Box::new(limit),
                    Box::new(offset),
                ],
            ),
        };

        let params: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match stmt.query_map(&params[..], |r| {
            Ok(EntitySummary {
                id: r.get(0)?,
                name: r.get(1)?,
                is_proper_name: r.get::<_, Option<i32>>(2)?.map(|v| v != 0),
                gender: r.get(3)?,
                start_date: r.get(4)?,
                number_of_references: r.get(5)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn list_places(
        &self,
        dataset_id: String,
        offset: i32,
        limit: i32,
        query: Option<String>,
    ) -> Vec<EntitySummary> {
        let Some(conn_arc) = read_or_recover(&self.conn).clone() else {
            return Vec::new();
        };
        let conn = lock_or_recover(&conn_arc);

        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::ToSql>>) = match query {
            Some(q) if !q.trim().is_empty() => (
                "SELECT id, name, NULL, NULL, feature_type,
                        (SELECT COUNT(*) FROM entity_refs
                         WHERE dataset_id=?1 AND source_kind='place' AND source_id=places.id)
                 FROM places WHERE dataset_id=?1 AND name LIKE ?2
                 ORDER BY name LIMIT ?3 OFFSET ?4"
                    .into(),
                vec![
                    Box::new(dataset_id.clone()),
                    Box::new(format!("%{}%", q)),
                    Box::new(limit),
                    Box::new(offset),
                ],
            ),
            _ => (
                "SELECT id, name, NULL, NULL, feature_type,
                        (SELECT COUNT(*) FROM entity_refs
                         WHERE dataset_id=?1 AND source_kind='place' AND source_id=places.id)
                 FROM places WHERE dataset_id=?1
                 ORDER BY name LIMIT ?2 OFFSET ?3"
                    .into(),
                vec![
                    Box::new(dataset_id.clone()),
                    Box::new(limit),
                    Box::new(offset),
                ],
            ),
        };

        let params: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match stmt.query_map(&params[..], |r| {
            Ok(EntitySummary {
                id: r.get(0)?,
                name: r.get(1)?,
                is_proper_name: None,
                gender: None,
                start_date: r.get(4)?,
                number_of_references: r.get(5)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn list_groups(
        &self,
        dataset_id: String,
        offset: i32,
        limit: i32,
        query: Option<String>,
    ) -> Vec<EntitySummary> {
        let Some(conn_arc) = read_or_recover(&self.conn).clone() else {
            return Vec::new();
        };
        let conn = lock_or_recover(&conn_arc);

        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::ToSql>>) = match query {
            Some(q) if !q.trim().is_empty() => (
                "SELECT id, name, NULL, NULL, NULL,
                        (SELECT COUNT(*) FROM entity_refs
                         WHERE dataset_id=?1 AND source_kind='group' AND source_id=people_groups.id)
                 FROM people_groups WHERE dataset_id=?1 AND name LIKE ?2
                 ORDER BY name LIMIT ?3 OFFSET ?4"
                    .into(),
                vec![
                    Box::new(dataset_id.clone()),
                    Box::new(format!("%{}%", q)),
                    Box::new(limit),
                    Box::new(offset),
                ],
            ),
            _ => (
                "SELECT id, name, NULL, NULL, NULL,
                        (SELECT COUNT(*) FROM entity_refs
                         WHERE dataset_id=?1 AND source_kind='group' AND source_id=people_groups.id)
                 FROM people_groups WHERE dataset_id=?1
                 ORDER BY name LIMIT ?2 OFFSET ?3"
                    .into(),
                vec![
                    Box::new(dataset_id.clone()),
                    Box::new(limit),
                    Box::new(offset),
                ],
            ),
        };

        let params: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match stmt.query_map(&params[..], |r| {
            Ok(EntitySummary {
                id: r.get(0)?,
                name: r.get(1)?,
                is_proper_name: None,
                gender: None,
                start_date: None,
                number_of_references: r.get(5)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    // ── Entity graph reads ───────────────────────────────────────

    pub fn get_verse_refs(&self, dataset_id: String, kind: String, id: String) -> Vec<VerseRange> {
        let Some(conn_arc) = read_or_recover(&self.conn).clone() else {
            return Vec::new();
        };
        let conn = lock_or_recover(&conn_arc);
        read_verse_refs(&conn, &dataset_id, &kind, &id)
    }

    pub fn get_related(
        &self,
        dataset_id: String,
        kind: String,
        id: String,
        relation: Option<String>,
    ) -> Vec<EntityRef> {
        let Some(conn_arc) = read_or_recover(&self.conn).clone() else {
            return Vec::new();
        };
        let conn = lock_or_recover(&conn_arc);

        match relation {
            Some(r) => read_forward_refs(&conn, &dataset_id, &kind, &id, &[r.as_str()]),
            None => {
                // No relation filter — return every outgoing edge.
                read_forward_refs_all(&conn, &dataset_id, &kind, &id)
            }
        }
    }

    pub fn get_entities_for_verse(
        &self,
        dataset_id: String,
        book: String,
        chapter: i32,
        verse: i32,
    ) -> Vec<EntityRef> {
        let Some(conn_arc) = read_or_recover(&self.conn).clone() else {
            return Vec::new();
        };
        let conn = lock_or_recover(&conn_arc);

        let mut stmt = match conn.prepare(
            "SELECT source_kind, source_id,
                    COALESCE((SELECT name FROM people WHERE id = source_id AND dataset_id = ?1),
                            (SELECT name FROM events WHERE id = source_id AND dataset_id = ?1),
                            (SELECT name FROM places WHERE id = source_id AND dataset_id = ?1),
                            (SELECT name FROM people_groups WHERE id = source_id AND dataset_id = ?1),
                            source_id)
            FROM entity_verse_refs
            WHERE dataset_id = ?1 AND book = ?2 AND chapter = ?3
            AND ?4 >= verse AND (?4 <= end_verse OR end_verse IS NULL)
            ORDER BY source_kind, source_id",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        match stmt.query_map(rusqlite::params![dataset_id, book, chapter, verse], |r| {
            Ok(EntityRef {
                kind: r.get(0)?,
                id: r.get(1)?,
                name: r.get(2)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn search_entities(&self, dataset_id: String, query: String) -> Vec<EntitySummary> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let Some(conn_arc) = read_or_recover(&self.conn).clone() else {
            return Vec::new();
        };
        let conn = lock_or_recover(&conn_arc);

        let pattern = format!("%{}%", query);
        let mut stmt = match conn.prepare(
            "SELECT id, name, NULL, NULL, NULL, 0 FROM people
             WHERE dataset_id = ?1 AND name LIKE ?2
             UNION ALL
             SELECT id, name, NULL, NULL, start_date, 0 FROM events
             WHERE dataset_id = ?1 AND name LIKE ?2
             UNION ALL
             SELECT id, name, NULL, NULL, NULL, 0 FROM places
             WHERE dataset_id = ?1 AND name LIKE ?2
             UNION ALL
             SELECT id, name, NULL, NULL, NULL, 0 FROM people_groups
             WHERE dataset_id = ?1 AND name LIKE ?2
             ORDER BY 2 LIMIT 200",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        match stmt.query_map(rusqlite::params![dataset_id, pattern], |r| {
            Ok(EntitySummary {
                id: r.get(0)?,
                name: r.get(1)?,
                is_proper_name: None,
                gender: None,
                start_date: r.get(4)?,
                number_of_references: r.get(5)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    // ── Install / uninstall ──────────────────────────────────────

    pub fn install_translation(&self, translation_id: String) -> Result<String, FreeBibleError> {
        let conn_arc =
            read_or_recover(&self.conn)
                .clone()
                .ok_or_else(|| FreeBibleError::Generic {
                    error_message: "call set_db_path first".into(),
                })?;

        let task_id = alloc_task_id(&self.next_id, "install", &translation_id);
        let cancel = Arc::new(AtomicBool::new(false));

        register_task(&self.tasks, &task_id, &translation_id, "Queued", &cancel);

        let tid = task_id.clone();
        let tasks = self.tasks.clone();
        let client = BibleClient::new();
        let id = translation_id.clone();

        thread::spawn(move || {
            set_state(&tasks, &tid, FreeBibleTaskState::Running, |p| {
                p.message = format!("Installing {}…", id);
            });

            let mut conn = lock_or_recover(&conn_arc);

            let result = install::install_translation(
                &client,
                &mut conn,
                &id,
                |p: InstallProgress| {
                    if let Some(t) = lock_or_recover(&tasks).get_mut(&tid) {
                        t.status.progress = p;
                    }
                },
                cancel.clone(),
            );

            drop(conn);

            match result {
                Ok(_) => {
                    info!("install {} complete", id);
                    set_state(&tasks, &tid, FreeBibleTaskState::Completed, |p| {
                        p.fraction = 1.0;
                        p.message = format!("{} installed", id);
                    });
                }
                Err(e) if is_cancelled(&e) => {
                    set_state(&tasks, &tid, FreeBibleTaskState::Cancelled, |p| {
                        p.message = "Cancelled".into();
                    });
                }
                Err(e) => {
                    let msg = format_err(&e);
                    error!("install {} failed: {}", id, msg);
                    set_state(
                        &tasks,
                        &tid,
                        FreeBibleTaskState::Failed { error: msg.clone() },
                        |p| {
                            p.message = format!("Install failed: {}", msg);
                        },
                    );
                }
            }
        });

        Ok(task_id)
    }

    pub fn uninstall_translation(&self, translation_id: String) -> Result<String, FreeBibleError> {
        let conn_arc =
            read_or_recover(&self.conn)
                .clone()
                .ok_or_else(|| FreeBibleError::Generic {
                    error_message: "call set_db_path first".into(),
                })?;

        let task_id = alloc_task_id(&self.next_id, "uninstall", &translation_id);
        let cancel = Arc::new(AtomicBool::new(false));

        register_task(&self.tasks, &task_id, &translation_id, "Queued", &cancel);

        let tid = task_id.clone();
        let tasks = self.tasks.clone();
        let id = translation_id.clone();

        thread::spawn(move || {
            set_state(&tasks, &tid, FreeBibleTaskState::Running, |p| {
                p.message = format!("Uninstalling {}…", id);
            });

            let mut conn = lock_or_recover(&conn_arc);
            let result = install::uninstall_translation(&mut conn, &id);
            drop(conn);

            match result {
                Ok(_) => {
                    info!("uninstall {} complete", id);
                    set_state(&tasks, &tid, FreeBibleTaskState::Completed, |p| {
                        p.fraction = 1.0;
                        p.message = format!("{} uninstalled", id);
                    });
                }
                Err(e) => {
                    let msg = format_err(&e);
                    error!("uninstall {} failed: {}", id, msg);
                    set_state(
                        &tasks,
                        &tid,
                        FreeBibleTaskState::Failed { error: msg.clone() },
                        |p| {
                            p.message = format!("Uninstall failed: {}", msg);
                        },
                    );
                }
            }
        });

        Ok(task_id)
    }

    // ── Task polling ─────────────────────────────────────────────

    pub fn poll_task(&self, task_id: String) -> Option<FreeBibleTaskStatus> {
        lock_or_recover(&self.tasks)
            .get(&task_id)
            .map(|t| t.status.clone())
    }

    pub fn cancel_task(&self, task_id: String) {
        if let Some(t) = lock_or_recover(&self.tasks).get(&task_id) {
            t.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn list_tasks(&self) -> Vec<FreeBibleTaskStatus> {
        lock_or_recover(&self.tasks)
            .values()
            .map(|t| t.status.clone())
            .collect()
    }

    // ── Installed state ──────────────────────────────────────────

    pub fn is_installed(&self, translation_id: String) -> bool {
        let Some(conn_arc) = read_or_recover(&self.conn).clone() else {
            return false;
        };
        let conn = lock_or_recover(&conn_arc);
        conn.query_row(
            "SELECT installed FROM translations WHERE id = ?1",
            rusqlite::params![translation_id],
            |r| r.get::<_, i32>(0),
        )
        .map(|v| v == 1)
        .unwrap_or(false)
    }
}
