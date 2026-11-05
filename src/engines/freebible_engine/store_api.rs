use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use log::{error, info};

use crate::db;
use crate::engines::freebible_engine::client::{
    BibleClient, FreeBibleBook, FreeBibleChapterItem, FreeBibleFootnote, FreeBibleVerse,
};
use crate::engines::freebible_engine::install;
use crate::engines::freebible_engine::store::{LanguageFacet, Store, StoreItem, StoreShelf};
use crate::engines::freebible_engine::utils::{
    FreeBibleError, FreeBibleTaskState, FreeBibleTaskStatus, InstallProgress,
};

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
