// freebible_engine/store_api.rs

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use log::{error, info};
use rusqlite::Connection;

use crate::db::writer::DbWriter;
use crate::engines::freebible_engine::error::FreeBibleError;
use crate::engines::freebible_engine::values::{TaskId, TaskKind};

use super::client::BibleClient;
use super::install::{self, EntityKind};
use super::store::{Store, StoreItem, StoreShelf};
use super::utils::{FreeBibleTaskState, FreeBibleTaskStatus, InstallProgress};
use super::values::{Flag, StoreItemId, StoreItemKind};

// ── Newtypes / bookkeeping ─────────────────────────────────────

#[derive(Debug)]
struct TaskCounter(u64);

#[allow(dead_code)]
#[derive(Debug)]
struct TaskEntry {
    kind: TaskKind,
    target: String,
    status: FreeBibleTaskStatus,
    cancel: Arc<AtomicBool>,
}

// ── FreeBibleStore ─────────────────────────────────────────────

/// FFI-visible store handle.
///
/// Reads go through `conn` (shared with the reader); writes go
/// through `writer` (single-threaded queue). A store that has been
/// constructed but not `attach_connection`d has neither and every
/// mutating method returns `not_open_err`.
#[derive(Debug, uniffi::Object)]
pub struct FreeBibleStore {
    conn: Arc<RwLock<Option<Arc<Mutex<Connection>>>>>,
    writer: RwLock<Option<DbWriter>>,
    inner: RwLock<Option<Arc<Store>>>,
    client: BibleClient,
    tasks: Arc<Mutex<HashMap<String, TaskEntry>>>,
    next_id: Arc<Mutex<TaskCounter>>,
}

// ── Lock helpers ───────────────────────────────────────────────

fn lock_or_recover<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn read_or_recover<T>(m: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(|p| p.into_inner())
}

fn write_or_recover<T>(m: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    m.write().unwrap_or_else(|p| p.into_inner())
}

// ── Task helpers ───────────────────────────────────────────────

fn alloc_task_id(next_id: &Arc<Mutex<TaskCounter>>, kind: TaskKind, target: &str) -> TaskId {
    let mut counter = lock_or_recover(next_id);
    let task_id = format!("{}_{}_{}", kind.as_wire(), target, counter.0);
    counter.0 += 1;
    TaskId(task_id)
}

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

// ── Task-thread scaffolding ────────────────────────────────────

/// Run `body` on a freshly spawned thread, mapping every outcome
/// (success, error, cancellation, panic, spawn failure) to a
/// terminal task state. All three install paths share this so the
/// failure handling exists exactly once.
fn run_task<F>(
    tasks: &Arc<Mutex<HashMap<String, TaskEntry>>>,
    tid: &TaskId,
    thread_name: &str,
    success_message: String,
    failure_prefix: &'static str,
    body: F,
) where
    F: FnOnce() -> Result<(), FreeBibleError> + Send + 'static,
{
    let tasks_for_thread = tasks.clone();
    let tid_for_thread = tid.clone();

    let spawned = thread::Builder::new()
        .name(thread_name.into())
        .spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(body));

            match outcome {
                Ok(Ok(())) => set_state(
                    &tasks_for_thread,
                    &tid_for_thread,
                    FreeBibleTaskState::Completed,
                    |p| {
                        p.fraction = 1.0;
                        p.message = success_message;
                    },
                ),
                Ok(Err(e)) if is_cancelled(&e) => set_state(
                    &tasks_for_thread,
                    &tid_for_thread,
                    FreeBibleTaskState::Cancelled,
                    |p| p.message = "Cancelled".into(),
                ),
                Ok(Err(e)) => {
                    let msg = format_err(&e);
                    error!("{failure_prefix}: {msg}");
                    set_state(
                        &tasks_for_thread,
                        &tid_for_thread,
                        FreeBibleTaskState::Failed { error: msg.clone() },
                        |p| p.message = format!("{failure_prefix}: {msg}"),
                    );
                }
                Err(_) => {
                    error!("{failure_prefix}: internal panic");
                    set_state(
                        &tasks_for_thread,
                        &tid_for_thread,
                        FreeBibleTaskState::Failed {
                            error: "internal panic".into(),
                        },
                        |p| p.message = format!("{failure_prefix}: internal panic"),
                    );
                }
            }
        });

    if let Err(e) = spawned {
        error!("spawn {thread_name} failed: {e}");
        set_state(
            tasks,
            tid,
            FreeBibleTaskState::Failed {
                error: format!("spawn {thread_name}: {e}"),
            },
            |p| p.message = format!("{failure_prefix}: could not start worker"),
        );
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

// ── Installed-flag overlay ─────────────────────────────────────

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
            writer: RwLock::new(None),
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

    pub fn install_translation(&self, translation_id: String) -> Result<String, FreeBibleError> {
        let writer = read_or_recover(&self.writer).clone().ok_or_else(not_open_err)?;

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
        let id = translation_id.clone();

        let success_message = format!("{} installed", id);
        run_task(
            &self.tasks,
            &task_id,
            "xbible-inst",
            success_message,
            "Install failed",
            move || {
                let outcome = run_install_translation(&writer, &id, &tasks, &tid, &cancel);
                outcome
            },
        );

        Ok(task_id.0)
    }

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

        let writer = read_or_recover(&self.writer).clone().ok_or_else(not_open_err)?;

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
        let dataset = dataset_id.clone();
        let kind_label = kind_str.clone();

        let success_message = format!("{} installed", kind_label);
        run_task(
            &self.tasks,
            &task_id,
            "xbible-entity",
            success_message,
            "Entity install failed",
            move || run_install_entity_set(&writer, &dataset, kind, &tasks, &tid, &cancel),
        );

        Ok(task_id.0)
    }

    pub fn uninstall_translation(&self, translation_id: String) -> Result<String, FreeBibleError> {
        let writer = read_or_recover(&self.writer).clone().ok_or_else(not_open_err)?;

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

        let success_message = format!("{} uninstalled", id);
        run_task(
            &self.tasks,
            &task_id,
            "xbible-uninst",
            success_message,
            "Uninstall failed",
            move || {
                set_state(&tasks, &tid, FreeBibleTaskState::Running, |p| {
                    p.message = format!("Uninstalling {}…", id);
                });

                let id_for_job = id.clone();
                writer.submit("uninstall", cancel.clone(), move |conn| {
                    install::uninstall_translation(conn, &id_for_job)
                })?;

                info!("uninstall {} complete", id);
                Ok(())
            },
        );

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

// ── Task-thread bodies ────────────────────────────────────────

fn run_install_translation(
    writer: &DbWriter,
    id: &str,
    tasks: &Arc<Mutex<HashMap<String, TaskEntry>>>,
    tid: &TaskId,
    cancel: &Arc<AtomicBool>,
) -> Result<(), FreeBibleError> {
    set_state(tasks, tid, FreeBibleTaskState::Running, |p| {
        p.message = format!("Installing {}…", id);
    });

    // Phase 1: network — no lock, no transaction.
    let client = BibleClient::new();
    let complete = client.complete(id, |received, total| {
        let fraction = total
            .and_then(|t| {
                if t > 0 {
                    Some(received as f64 / t as f64 * 0.8)
                } else {
                    None
                }
            })
            .unwrap_or(0.0);
        update_progress(
            tasks,
            tid,
            InstallProgress {
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
                        id,
                        received as f64 / 1_048_576.0,
                        t as f64 / 1_048_576.0,
                    ),
                    None => format!(
                        "Downloading {} — {:.1} MB",
                        id,
                        received as f64 / 1_048_576.0,
                    ),
                },
            },
        );
    })?;

    // Phase 2: write — small jobs, each its own transaction.
    let total_chapters: i32 = complete.books.iter().map(|b| b.chapters.len() as i32).sum();
    let id_owned = id.to_string();

    writer.submit("begin_install", cancel.clone(), {
        let id = id_owned.clone();
        move |conn| install::begin_install(conn, &id)
    })?;

    let mut chapters_done = 0i32;
    let mut verses_done = 0i32;
    for book in complete.books {
        let book_for_job = book.clone();
        let id = id_owned.clone();
        writer.submit("persist_book", cancel.clone(), move |conn| {
            install::persist_book(conn, &id, &book_for_job)
        })?;

        chapters_done += book.chapters.len() as i32;
        verses_done += book
            .chapters
            .iter()
            .map(|c| c.verses.len() as i32)
            .sum::<i32>();

        let fraction = 0.8 + (chapters_done as f64 / total_chapters.max(1) as f64) * 0.2;
        update_progress(
            tasks,
            tid,
            InstallProgress {
                fraction,
                chapters_done,
                chapters_total: total_chapters,
                current_book_name: book.name.clone(),
                current_book_id: book.id.clone(),
                current_chapter: book.chapters.last().map(|c| c.number).unwrap_or(0),
                current_chapter_verses: 0,
                verses_done,
                message: format!(
                    "{} — {}/{} chapters, {} verses",
                    book.name, chapters_done, total_chapters, verses_done
                ),
            },
        );
    }

    writer.submit("mark_installed", cancel.clone(), {
        let id = id_owned.clone();
        move |conn| install::mark_installed(conn, &id)
    })?;

    info!(
        "installed {} ({} chapters, {} verses)",
        id, chapters_done, verses_done
    );
    Ok(())
}

fn run_install_entity_set(
    writer: &DbWriter,
    dataset_id: &str,
    kind: EntityKind,
    tasks: &Arc<Mutex<HashMap<String, TaskEntry>>>,
    tid: &TaskId,
    cancel: &Arc<AtomicBool>,
) -> Result<(), FreeBibleError> {
    set_state(tasks, tid, FreeBibleTaskState::Running, |p| {
        p.message = format!("Preparing {} install…", kind.as_str());
    });

    // Phase 1: network — fetch all details up front; the writer
    // never sees a connection until batch-submit time.
    let client = BibleClient::new();
    let total = install::count_entity_set(&client, dataset_id, kind)?;
    let mut fetched = install::fetch_entity_set(&client, dataset_id, kind, |done| {
        let fraction = if total > 0 {
            (done as f64 / total as f64) * 0.8
        } else {
            0.0
        };
        update_progress(
            tasks,
            tid,
            InstallProgress {
                fraction,
                chapters_done: done,
                chapters_total: total,
                current_book_name: String::new(),
                current_book_id: String::new(),
                current_chapter: 0,
                current_chapter_verses: 0,
                verses_done: done,
                message: format!("Fetching {} ({}/{})", kind.as_str(), done, total),
            },
        );
    })?;

    // Phase 2: write — wipe, then batches of 500.
    let dataset_owned = dataset_id.to_string();
    let kind_for_job = kind;
    writer.submit("begin_entity_install", cancel.clone(), {
        let dataset = dataset_owned.clone();
        move |conn| install::begin_entity_install(conn, &dataset, kind_for_job)
    })?;

    let batch_size = 500;
    let mut written = 0i32;
    while !fetched.is_empty() {
        let take = fetched.len().min(batch_size);
        let batch: Vec<_> = fetched.drain(..take).collect();
        let dataset = dataset_owned.clone();
        let kind_for_job = kind;
        writer.submit("persist_entity_batch", cancel.clone(), move |conn| {
            install::persist_entity_batch(conn, &dataset, kind_for_job, &batch)
        })?;

        written += take as i32;
        let fraction = 0.8 + (written as f64 / total.max(1) as f64) * 0.2;
        update_progress(
            tasks,
            tid,
            InstallProgress {
                fraction,
                chapters_done: written,
                chapters_total: total,
                current_book_name: String::new(),
                current_book_id: String::new(),
                current_chapter: 0,
                current_chapter_verses: 0,
                verses_done: written,
                message: format!("Writing {} ({}/{})", kind.as_str(), written, total),
            },
        );
    }

    writer.submit("mark_entity_installed", cancel.clone(), {
        let dataset = dataset_owned.clone();
        move |conn| install::mark_entity_installed(conn, &dataset, kind, written, total)
    })?;

    info!("entity set {}:{} installed", dataset_id, kind.as_str());
    Ok(())
}

// ── Internal API — only the engine calls this ─────────────────

impl FreeBibleStore {
    pub(crate) fn attach_connection(&self, conn: Arc<Mutex<Connection>>, writer: DbWriter) {
        *write_or_recover(&self.conn) = Some(conn);
        *write_or_recover(&self.writer) = Some(writer);
    }

    /// Cancel every queued or running task. Called by the engine
    /// before it shuts down the writer on `set_db_path` switching
    /// databases.
    pub(crate) fn cancel_all_tasks(&self) {
        for t in lock_or_recover(&self.tasks).values() {
            t.cancel.store(true, Ordering::Relaxed);
        }
    }
}