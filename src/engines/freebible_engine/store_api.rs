// freebible_engine/store_api.rs

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use log::{error, info};
use rusqlite::Connection;

use crate::engines::freebible_engine::error::FreeBibleError;
use crate::engines::freebible_engine::values::{TaskId, TaskKind};

use super::client::BibleClient;
use super::install::{self, EntityKind};
use super::store::{Store, StoreItem, StoreShelf};
use super::utils::{FreeBibleTaskState, FreeBibleTaskStatus, InstallProgress};
use super::values::{Flag, StoreItemId, StoreItemKind};

// ── Newtypes for store_api ─────────────────────────────────────

/// Wrapper around `u64` used to allocate task IDs. Distinct type so
/// it can never be confused with a chapter number or a book count.
#[derive(Debug)]
struct TaskCounter(u64);

// ── Task bookkeeping ───────────────────────────────────────────

#[allow(dead_code)]
#[derive(Debug)]
struct TaskEntry {
    kind: TaskKind,
    target: String,
    status: FreeBibleTaskStatus,
    cancel: Arc<AtomicBool>,
}

// ── FreeBibleStore ─────────────────────────────────────────────

/// FFI-visible store handle. The concrete `Store` catalog only exists
/// when the engine is in `Open` state, so methods take `&self` and
/// reach into the shared handle (single runtime check at the boundary).
#[derive(Debug, uniffi::Object)]
pub struct FreeBibleStore {
    conn: Arc<RwLock<Option<Arc<Mutex<Connection>>>>>,
    inner: RwLock<Option<Arc<Store>>>,
    client: BibleClient,
    tasks: Arc<Mutex<HashMap<String, TaskEntry>>>,
    next_id: Arc<Mutex<TaskCounter>>,
}

// ── Locks that never panic on poison ───────────────────────────

fn lock_or_recover<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn read_or_recover<T>(m: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(|p| p.into_inner())
}

fn write_or_recover<T>(m: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    m.write().unwrap_or_else(|p| p.into_inner())
}

// ── Task ID allocation ─────────────────────────────────────────

fn alloc_task_id(next_id: &Arc<Mutex<TaskCounter>>, kind: TaskKind, target: &str) -> TaskId {
    let mut counter = lock_or_recover(next_id);
    let task_id = format!("{}_{}_{}", kind.as_wire(), target, counter.0);
    counter.0 += 1;
    TaskId(task_id)
}

// ── Task registration helpers ──────────────────────────────────

fn register_task(
    tasks: &Arc<Mutex<HashMap<String, TaskEntry>>>,
    task_id: &TaskId,
    kind: TaskKind,
    target: &str,
    message: &str,
    cancel: &Arc<AtomicBool>,
) {
    let entry = TaskEntry {
        kind,
        target: target.to_string(),
        status: FreeBibleTaskStatus {
            task_id: task_id.0.clone(),
            state: FreeBibleTaskState::Queued,
            progress: empty_progress(format!("{} {}", message, target)),
        },
        cancel: cancel.clone(),
    };
    lock_or_recover(tasks).insert(task_id.0.clone(), entry);
}

fn set_state<F>(
    tasks: &Arc<Mutex<HashMap<String, TaskEntry>>>,
    task_id: &TaskId,
    state: FreeBibleTaskState,
    mutate: F,
) where
    F: FnOnce(&mut InstallProgress),
{
    if let Some(t) = lock_or_recover(tasks).get_mut(&task_id.0) {
        t.status.state = state;
        mutate(&mut t.status.progress);
    }
}

fn update_progress(
    tasks: &Arc<Mutex<HashMap<String, TaskEntry>>>,
    task_id: &TaskId,
    progress: InstallProgress,
) {
    if let Some(t) = lock_or_recover(tasks).get_mut(&task_id.0) {
        t.status.progress = progress;
    }
}

fn empty_progress(message: String) -> InstallProgress {
    InstallProgress {
        fraction: 0.0,
        chapters_done: 0,
        chapters_total: 0,
        current_book_name: String::new(),
        current_book_id: String::new(),
        current_chapter: 0,
        current_chapter_verses: 0,
        verses_done: 0,
        message,
    }
}

// ── Error helpers ──────────────────────────────────────────────

fn is_cancelled(err: &FreeBibleError) -> bool {
    matches!(err, FreeBibleError::Generic { error_message } if error_message == "cancelled")
}

fn format_err(err: &FreeBibleError) -> String {
    match err {
        FreeBibleError::Generic { error_message } => error_message.clone(),
    }
}

fn not_open_err() -> FreeBibleError {
    FreeBibleError::Generic {
        error_message: "engine not open — call setDbPath first".into(),
    }
}

// ── Entity install state overlay ───────────────────────────────

/// Overlay the current installed flag on every item in a shelf list.
/// Skips the SQLite round-trip if a task is running (the UI already
/// shows an installing state for that item).
fn overlay_installed_flags(
    conn: &Connection,
    tasks: &Arc<Mutex<HashMap<String, TaskEntry>>>,
    items: &mut [StoreItem],
) {
    let running = {
        let tasks = lock_or_recover(tasks);
        tasks.values().any(|t| {
            matches!(
                t.status.state,
                FreeBibleTaskState::Queued | FreeBibleTaskState::Running
            )
        })
    };
    if running {
        return;
    }

    for item in items.iter_mut() {
        let Some(kind) = StoreItemKind::from_wire(&item.kind) else {
            continue;
        };
        match kind {
            StoreItemKind::EntitySet => {
                if let Some((dataset, entity_kind)) = item.id.split_once(':') {
                    let installed: i32 = conn
                        .query_row(
                            "SELECT installed FROM entity_install_state
                             WHERE dataset_id = ?1 AND kind = ?2",
                            rusqlite::params![dataset, entity_kind],
                            |r| r.get(0),
                        )
                        .unwrap_or(0);
                    item.installed = Flag::from_sql_int(installed).as_bool();
                }
            }
            StoreItemKind::Translation => {
                let installed: i32 = conn
                    .query_row(
                        "SELECT installed FROM translations WHERE id = ?1",
                        rusqlite::params![item.id],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                item.installed = Flag::from_sql_int(installed).as_bool();
            }
            // Commentaries and datasets don't have a per-kind installed
            // table yet — they're always "not installed" from the store's
            // point of view.
            StoreItemKind::Commentary | StoreItemKind::Dataset => {}
        }
    }
}

// ── UniFFI export block ────────────────────────────────────────

#[uniffi::export]
impl FreeBibleStore {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            conn: Arc::new(RwLock::new(None)),
            inner: RwLock::new(None),
            client: BibleClient::new(),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(TaskCounter(0))),
        })
    }

    // ── Catalog ────────────────────────────────────────────────

    pub fn refresh(&self) -> Result<(), FreeBibleError> {
        let conn_arc = read_or_recover(&self.conn).clone();
        let store = if let Some(conn_arc) = &conn_arc {
            let conn = lock_or_recover(conn_arc);
            Store::fetch_index(&self.client, Some(&conn))
        } else {
            Store::fetch_index(&self.client, None)
        }
        .map_err(|e| FreeBibleError::Generic { error_message: e })?;

        *write_or_recover(&self.inner) = Some(Arc::new(store));
        info!("catalog index refreshed");
        Ok(())
    }

    pub fn home_feed(&self, language: Option<String>) -> Vec<StoreShelf> {
        let Some(store) = read_or_recover(&self.inner).clone() else {
            return Vec::new();
        };

        let mut shelves = store.home_feed(language.as_deref());

        if let Some(conn_arc) = read_or_recover(&self.conn).clone() {
            let conn = lock_or_recover(&conn_arc);
            for shelf in &mut shelves {
                overlay_installed_flags(&conn, &self.tasks, &mut shelf.items);
            }
        }

        shelves
    }

    pub fn search(&self, query: String) -> Vec<StoreItem> {
        read_or_recover(&self.inner)
            .as_ref()
            .map(|s| s.search(&query))
            .unwrap_or_default()
    }

    pub fn languages(&self) -> Vec<super::store::LanguageFacet> {
        read_or_recover(&self.inner)
            .as_ref()
            .map(|s| s.languages())
            .unwrap_or_default()
    }

    pub fn item(&self, id: String, kind: String) -> Option<StoreItem> {
        let kind = StoreItemKind::from_wire(&kind)?;
        let item_id = StoreItemId { kind, raw: id };

        read_or_recover(&self.inner)
            .as_ref()
            .and_then(|s| s.item(&item_id))
            .map(StoreItem::from)
    }

    // ── Shelf paging ───────────────────────────────────────────

    pub fn get_shelf(
        &self,
        shelf_id: String,
        language: Option<String>,
        offset: u32,
        limit: u32,
    ) -> Vec<StoreItem> {
        let Some(store) = read_or_recover(&self.inner).clone() else {
            return Vec::new();
        };

        let mut items = store.shelf_page(&shelf_id, language.as_deref(), offset, limit);

        if let Some(conn_arc) = read_or_recover(&self.conn).clone() {
            let conn = lock_or_recover(&conn_arc);
            overlay_installed_flags(&conn, &self.tasks, &mut items);
        }

        items
    }

    pub fn shelf_total(&self, shelf_id: String, language: Option<String>) -> u32 {
        read_or_recover(&self.inner)
            .as_ref()
            .map(|s| s.shelf_total(&shelf_id, language.as_deref()))
            .unwrap_or(0)
    }

    // ── Install ────────────────────────────────────────────────

    pub fn install_entity_set(
        &self,
        dataset_id: String,
        kind_str: String,
    ) -> Result<String, FreeBibleError> {
        let Some(kind) = EntityKind::from_wire(&kind_str) else {
            return Err(FreeBibleError::Generic {
                error_message: format!("unknown entity kind: {kind_str}"),
            });
        };

        let conn_arc = read_or_recover(&self.conn)
            .clone()
            .ok_or_else(not_open_err)?;

        let task_id = alloc_task_id(&self.next_id, TaskKind::Entity, &kind_str);
        let cancel = Arc::new(AtomicBool::new(false));

        register_task(
            &self.tasks,
            &task_id,
            TaskKind::Entity,
            &kind_str,
            "Queued",
            &cancel,
        );

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
                |p| update_progress(&tasks, &tid, p),
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
                        |p| p.message = format!("Install failed: {msg}"),
                    );
                }
            }
        });

        Ok(task_id.0)
    }

    pub fn install_translation(&self, translation_id: String) -> Result<String, FreeBibleError> {
        let conn_arc = read_or_recover(&self.conn)
            .clone()
            .ok_or_else(not_open_err)?;

        let task_id = alloc_task_id(&self.next_id, TaskKind::Install, &translation_id);
        let cancel = Arc::new(AtomicBool::new(false));

        register_task(
            &self.tasks,
            &task_id,
            TaskKind::Install,
            &translation_id,
            "Queued",
            &cancel,
        );

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
                |p| update_progress(&tasks, &tid, p),
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
                        |p| p.message = format!("Install failed: {msg}"),
                    );
                }
            }
        });

        Ok(task_id.0)
    }

    pub fn uninstall_translation(&self, translation_id: String) -> Result<String, FreeBibleError> {
        let conn_arc = read_or_recover(&self.conn)
            .clone()
            .ok_or_else(not_open_err)?;

        let task_id = alloc_task_id(&self.next_id, TaskKind::Uninstall, &translation_id);
        let cancel = Arc::new(AtomicBool::new(false));

        register_task(
            &self.tasks,
            &task_id,
            TaskKind::Uninstall,
            &translation_id,
            "Queued",
            &cancel,
        );

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
                        |p| p.message = format!("Uninstall failed: {msg}"),
                    );
                }
            }
        });

        Ok(task_id.0)
    }

    // ── Task polling ───────────────────────────────────────────

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
}

// ── Internal API — only the engine calls this ─────────────────

impl FreeBibleStore {
    /// Called by `FreeBibleEngine::set_db_path`. Stores the shared
    /// connection Arc so future reads/writes go to the same database.
    pub(crate) fn attach_connection(&self, conn: Arc<Mutex<Connection>>) {
        *write_or_recover(&self.conn) = Some(conn);
    }
}
