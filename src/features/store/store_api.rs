//! FFI-visible Store feature handle backed by `SourceRegistry` and `DbWriter`
//! (`src/features/store/store_api.rs`).

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use log::{error, info};
use rusqlite::Connection;

use crate::db::install;
use crate::db::writer::DbWriter;
use crate::error::XBibleError;
use crate::features::store::store::{LanguageFacet, Store, StoreItem, StoreShelf};
use crate::sources::helloao::HelloAOSource;
use crate::sources::{BibleSource, GenericInstalledBible, InstallProgressSink, SourceRegistry};

// ── Progress & Task State Types (UniFFI) ──────────────────────────────────────

#[derive(Debug, Clone, uniffi::Record)]
pub struct InstallProgress {
    pub fraction: f64,
    pub chapters_done: i32,
    pub chapters_total: i32,
    pub current_book_name: String,
    pub current_book_id: String,
    pub current_chapter: i32,
    pub current_chapter_verses: i32,
    pub verses_done: i32,
    pub message: String,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum StoreTaskState {
    Queued,
    Running,
    Completed,
    Failed { error: String },
    Cancelled,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct StoreTaskStatus {
    pub task_id: String,
    pub state: StoreTaskState,
    pub progress: InstallProgress,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskKind {
    Install,
    Uninstall,
    Entity,
}

impl TaskKind {
    fn as_wire(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Uninstall => "uninstall",
            Self::Entity => "entity",
        }
    }
}

#[derive(Debug)]
struct TaskCounter(u64);

#[allow(dead_code)]
#[derive(Debug)]
struct TaskEntry {
    kind: TaskKind,
    target: String,
    status: StoreTaskStatus,
    cancel: Arc<AtomicBool>,
}

// ── InstallProgressSink Adapter ───────────────────────────────────────────────

/// Adapts generic `InstallProgressSink` callbacks from any `BibleSource` into
/// `InstallProgress` updates on `StoreApi`'s task map.
struct StoreProgressBridge {
    tasks: Arc<Mutex<HashMap<String, TaskEntry>>>,
    task_id: String,
    cancel: Arc<AtomicBool>,
    label: String,
}

impl InstallProgressSink for StoreProgressBridge {
    fn on_progress(&self, stage: String, current: u32, total: u32, message: Option<String>) {
        let base_fraction: f64 = match stage.as_str() {
            "catalog" => 0.05,
            "download" => {
                if total > 0 {
                    0.05 + (current as f64 / total as f64) * 0.70
                } else {
                    0.25
                }
            }
            "normalize" => 0.78,
            "done" => 0.80,
            _ => 0.10,
        };

        let display_message: String = message.unwrap_or_else(|| match stage.as_str() {
            "download" if total > 0 => {
                format!("Downloading {} ({}/{})", self.label, current, total)
            }
            "normalize" => format!("Structuring {}…", self.label),
            _ => format!("Installing {}…", self.label),
        });

        update_progress(
            &self.tasks,
            &self.task_id,
            InstallProgress {
                fraction: base_fraction,
                chapters_done: if stage == "download" {
                    current as i32
                } else {
                    0
                },
                chapters_total: if stage == "download" { total as i32 } else { 0 },
                current_book_name: String::new(),
                current_book_id: String::new(),
                current_chapter: 0,
                current_chapter_verses: 0,
                verses_done: 0,
                message: display_message,
            },
        );
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

// ── StoreApi (UniFFI Object) ──────────────────────────────────────────────────

#[derive(uniffi::Object)]
pub struct StoreApi {
    conn: Arc<RwLock<Option<Arc<Mutex<Connection>>>>>,
    writer: RwLock<Option<DbWriter>>,
    inner: RwLock<Option<Arc<Store>>>,
    registry: Arc<SourceRegistry>,
    helloao: Arc<HelloAOSource>,
    tasks: Arc<Mutex<HashMap<String, TaskEntry>>>,
    next_id: Arc<Mutex<TaskCounter>>,
}

impl std::fmt::Debug for StoreApi {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreApi")
            .field("source_count", &self.registry.all().len())
            .finish_non_exhaustive()
    }
}

// ── Lock & Task Helpers ───────────────────────────────────────────────────────

fn lock_or_recover<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn read_or_recover<T>(m: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    m.read().unwrap_or_else(|p| p.into_inner())
}

fn write_or_recover<T>(m: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    m.write().unwrap_or_else(|p| p.into_inner())
}

fn alloc_task_id(next_id: &Arc<Mutex<TaskCounter>>, kind: TaskKind, target: &str) -> String {
    let mut counter = lock_or_recover(next_id);
    let task_id: String = format!("{}_{}_{}", kind.as_wire(), target, counter.0);
    counter.0 += 1;
    task_id
}

fn register_task(
    tasks: &Arc<Mutex<HashMap<String, TaskEntry>>>,
    task_id: &str,
    kind: TaskKind,
    target: &str,
    message: &str,
    cancel: &Arc<AtomicBool>,
) {
    let entry: TaskEntry = TaskEntry {
        kind,
        target: target.to_string(),
        status: StoreTaskStatus {
            task_id: task_id.to_string(),
            state: StoreTaskState::Queued,
            progress: empty_progress(format!("{} {}", message, target)),
        },
        cancel: cancel.clone(),
    };
    lock_or_recover(tasks).insert(task_id.to_string(), entry);
}

fn set_state<F>(
    tasks: &Arc<Mutex<HashMap<String, TaskEntry>>>,
    task_id: &str,
    state: StoreTaskState,
    mutate: F,
) where
    F: FnOnce(&mut InstallProgress),
{
    if let Some(t) = lock_or_recover(tasks).get_mut(task_id) {
        t.status.state = state;
        mutate(&mut t.status.progress);
    }
}

fn update_progress(
    tasks: &Arc<Mutex<HashMap<String, TaskEntry>>>,
    task_id: &str,
    progress: InstallProgress,
) {
    if let Some(t) = lock_or_recover(tasks).get_mut(task_id) {
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

fn run_task<F>(
    tasks: &Arc<Mutex<HashMap<String, TaskEntry>>>,
    tid: &str,
    thread_name: &str,
    success_message: String,
    failure_prefix: &'static str,
    body: F,
) where
    F: FnOnce() -> Result<(), XBibleError> + Send + 'static,
{
    let tasks_for_thread = tasks.clone();
    let tid_for_thread: String = tid.to_string();

    let spawned = thread::Builder::new()
        .name(thread_name.into())
        .spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(body));

            match outcome {
                Ok(Ok(())) => set_state(
                    &tasks_for_thread,
                    &tid_for_thread,
                    StoreTaskState::Completed,
                    |p: &mut InstallProgress| {
                        p.fraction = 1.0;
                        p.message = success_message;
                    },
                ),
                Ok(Err(e)) if is_cancelled(&e) => set_state(
                    &tasks_for_thread,
                    &tid_for_thread,
                    StoreTaskState::Cancelled,
                    |p: &mut InstallProgress| p.message = "Cancelled".into(),
                ),
                Ok(Err(e)) => {
                    let msg: String = format_err(&e);
                    error!("{failure_prefix}: {msg}");
                    set_state(
                        &tasks_for_thread,
                        &tid_for_thread,
                        StoreTaskState::Failed { error: msg.clone() },
                        |p: &mut InstallProgress| p.message = format!("{failure_prefix}: {msg}"),
                    );
                }
                Err(_) => {
                    error!("{failure_prefix}: internal panic");
                    set_state(
                        &tasks_for_thread,
                        &tid_for_thread,
                        StoreTaskState::Failed {
                            error: "internal panic".into(),
                        },
                        |p: &mut InstallProgress| {
                            p.message = format!("{failure_prefix}: internal panic")
                        },
                    );
                }
            }
        });

    if let Err(e) = spawned {
        error!("spawn {thread_name} failed: {e}");
        set_state(
            tasks,
            tid,
            StoreTaskState::Failed {
                error: format!("spawn {thread_name}: {e}"),
            },
            |p: &mut InstallProgress| {
                p.message = format!("{failure_prefix}: could not start worker")
            },
        );
    }
}

fn is_cancelled(err: &XBibleError) -> bool {
    matches!(err, XBibleError::Generic { error_message } if error_message.to_lowercase().contains("cancel"))
}

fn format_err(err: &XBibleError) -> String {
    match err {
        XBibleError::Generic { error_message } => error_message.clone(),
    }
}

fn not_open_err() -> XBibleError {
    XBibleError::Generic {
        error_message: "engine not open — call setDbPath first".into(),
    }
}

fn overlay_installed_flags(
    conn: &Connection,
    tasks: &Arc<Mutex<HashMap<String, TaskEntry>>>,
    items: &mut [StoreItem],
) {
    let running: bool = {
        let tasks = lock_or_recover(tasks);
        tasks.values().any(|t: &TaskEntry| {
            matches!(
                t.status.state,
                StoreTaskState::Queued | StoreTaskState::Running
            )
        })
    };
    if running {
        return;
    }

    for item in items.iter_mut() {
        match item.kind.as_str() {
            "entity_set" => {
                if let Some((dataset, entity_kind)) = item.id.split_once(':') {
                    let installed: i32 = conn
                        .query_row(
                            "SELECT installed FROM entity_install_state
                             WHERE dataset_id = ?1 AND kind = ?2",
                            rusqlite::params![dataset, entity_kind],
                            |r| r.get(0),
                        )
                        .unwrap_or(0);
                    item.installed = installed != 0;
                }
            }
            "translation" => {
                let installed: i32 = conn
                    .query_row(
                        "SELECT installed FROM translations WHERE id = ?1",
                        rusqlite::params![item.id],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                item.installed = installed != 0;
            }
            "commentary" => {
                let installed: i32 = conn
                    .query_row(
                        "SELECT installed FROM commentaries WHERE id = ?1",
                        rusqlite::params![item.id],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                item.installed = installed != 0;
            }
            _ => {}
        }
    }
}

// ── UniFFI Export Block ───────────────────────────────────────────────────────

#[uniffi::export]
impl StoreApi {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        let registry: Arc<SourceRegistry> = Arc::new(SourceRegistry::new());
        let helloao: Arc<HelloAOSource> = Arc::new(HelloAOSource::new());
        registry.register(helloao.clone());

        Arc::new(Self {
            conn: Arc::new(RwLock::new(None)),
            writer: RwLock::new(None),
            inner: RwLock::new(None),
            registry,
            helloao,
            tasks: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(TaskCounter(0))),
        })
    }

    // ── Catalog ───────────────────────────────────────────────────────────────

    pub fn refresh(&self) -> Result<(), XBibleError> {
        let conn_arc = read_or_recover(&self.conn).clone();
        let store: Store = if let Some(conn_arc) = &conn_arc {
            let conn = lock_or_recover(conn_arc);
            Store::fetch_index(&self.registry, Some(&self.helloao), Some(&conn))
        } else {
            Store::fetch_index(&self.registry, Some(&self.helloao), None)
        }
        .map_err(|e: String| XBibleError::Generic { error_message: e })?;

        *write_or_recover(&self.inner) = Some(Arc::new(store));
        info!("catalog index refreshed across registered sources");
        Ok(())
    }

    pub fn home_feed(&self, language: Option<String>) -> Vec<StoreShelf> {
        let Some(store) = read_or_recover(&self.inner).clone() else {
            return Vec::new();
        };

        let mut shelves: Vec<StoreShelf> = store.home_feed(language.as_deref());

        if let Some(conn_arc) = read_or_recover(&self.conn).clone() {
            let conn = lock_or_recover(&conn_arc);
            for shelf in &mut shelves {
                overlay_installed_flags(&conn, &self.tasks, &mut shelf.items);
            }
        }

        shelves
    }

    pub fn search(&self, query: String) -> Vec<StoreItem> {
        let mut items: Vec<StoreItem> = read_or_recover(&self.inner)
            .as_ref()
            .map(|s: &Arc<Store>| s.search(&query))
            .unwrap_or_default();

        if let Some(conn_arc) = read_or_recover(&self.conn).clone() {
            let conn = lock_or_recover(&conn_arc);
            overlay_installed_flags(&conn, &self.tasks, &mut items);
        }

        items
    }

    pub fn languages(&self) -> Vec<LanguageFacet> {
        read_or_recover(&self.inner)
            .as_ref()
            .map(|s: &Arc<Store>| s.languages())
            .unwrap_or_default()
    }

    pub fn item(&self, id: String, kind: String) -> Option<StoreItem> {
        let mut item: StoreItem = read_or_recover(&self.inner)
            .as_ref()
            .and_then(|s: &Arc<Store>| s.item(&id, &kind))?;

        if let Some(conn_arc) = read_or_recover(&self.conn).clone() {
            let conn = lock_or_recover(&conn_arc);
            overlay_installed_flags(&conn, &self.tasks, std::slice::from_mut(&mut item));
        }

        Some(item)
    }

    // ── Shelf Paging ──────────────────────────────────────────────────────────

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

        let mut items: Vec<StoreItem> =
            store.shelf_page(&shelf_id, language.as_deref(), offset, limit);

        if let Some(conn_arc) = read_or_recover(&self.conn).clone() {
            let conn = lock_or_recover(&conn_arc);
            overlay_installed_flags(&conn, &self.tasks, &mut items);
        }

        items
    }

    pub fn shelf_total(&self, shelf_id: String, language: Option<String>) -> u32 {
        read_or_recover(&self.inner)
            .as_ref()
            .map(|s: &Arc<Store>| s.shelf_total(&shelf_id, language.as_deref()))
            .unwrap_or(0)
    }

    // ── Install / Uninstall: Translation (Source-Agnostic) ────────────────────

    pub fn install_translation(&self, translation_id: String) -> Result<String, XBibleError> {
        let source_id: String = read_or_recover(&self.inner)
            .as_ref()
            .and_then(|s: &Arc<Store>| s.source_for_translation(&translation_id))
            .unwrap_or_else(|| "helloao".to_string());

        self.install_translation_from_source(source_id, translation_id)
    }

    pub fn install_translation_from_source(
        &self,
        source_id: String,
        translation_id: String,
    ) -> Result<String, XBibleError> {
        if translation_id.contains(':') {
            return Err(XBibleError::Generic {
                error_message: format!(
                    "{translation_id} looks like a store item id, not a translation id — \
                     call installEntitySet instead"
                ),
            });
        }

        let source: Arc<dyn BibleSource> =
            self.registry
                .get(&source_id)
                .ok_or_else(|| XBibleError::Generic {
                    error_message: format!("unknown Bible source: {source_id}"),
                })?;

        let writer: DbWriter = read_or_recover(&self.writer)
            .clone()
            .ok_or_else(not_open_err)?;

        let task_id: String = alloc_task_id(&self.next_id, TaskKind::Install, &translation_id);
        let cancel: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

        register_task(
            &self.tasks,
            &task_id,
            TaskKind::Install,
            &translation_id,
            "Queued",
            &cancel,
        );

        let tid: String = task_id.clone();
        let tasks = self.tasks.clone();
        let id: String = translation_id.clone();
        let success_message: String = format!("{} installed", id);

        run_task(
            &self.tasks,
            &task_id,
            "xbible-inst",
            success_message,
            "Install failed",
            move || run_install_translation(&writer, source, &id, &tasks, &tid, &cancel),
        );

        Ok(task_id)
    }

    pub fn uninstall_translation(&self, translation_id: String) -> Result<String, XBibleError> {
        if translation_id.contains(':') {
            return Err(XBibleError::Generic {
                error_message: format!(
                    "{translation_id} looks like a store item id, not a translation id"
                ),
            });
        }

        let writer: DbWriter = read_or_recover(&self.writer)
            .clone()
            .ok_or_else(not_open_err)?;

        let task_id: String = alloc_task_id(&self.next_id, TaskKind::Uninstall, &translation_id);
        let cancel: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

        register_task(
            &self.tasks,
            &task_id,
            TaskKind::Uninstall,
            &translation_id,
            "Queued",
            &cancel,
        );

        let tid: String = task_id.clone();
        let tasks = self.tasks.clone();
        let id: String = translation_id.clone();
        let success_message: String = format!("{} uninstalled", id);

        run_task(
            &self.tasks,
            &task_id,
            "xbible-uninst",
            success_message,
            "Uninstall failed",
            move || {
                set_state(&tasks, &tid, StoreTaskState::Running, |p| {
                    p.message = format!("Uninstalling {}…", id);
                });

                let id_for_job: String = id.clone();
                writer.submit("uninstall", cancel.clone(), move |conn| {
                    install::uninstall_translation(conn, &id_for_job)
                })?;

                info!("uninstall {} complete", id);
                Ok(())
            },
        );

        Ok(task_id)
    }

    // ── Task Polling ──────────────────────────────────────────────────────────

    pub fn poll_task(&self, task_id: String) -> Option<StoreTaskStatus> {
        lock_or_recover(&self.tasks)
            .get(&task_id)
            .map(|t: &TaskEntry| t.status.clone())
    }

    pub fn cancel_task(&self, task_id: String) {
        if let Some(t) = lock_or_recover(&self.tasks).get(&task_id) {
            t.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn list_tasks(&self) -> Vec<StoreTaskStatus> {
        lock_or_recover(&self.tasks)
            .values()
            .map(|t: &TaskEntry| t.status.clone())
            .collect()
    }
}

// ── Background Task Execution ─────────────────────────────────────────────────

fn run_install_translation(
    writer: &DbWriter,
    source: Arc<dyn BibleSource>,
    id: &str,
    tasks: &Arc<Mutex<HashMap<String, TaskEntry>>>,
    tid: &str,
    cancel: &Arc<AtomicBool>,
) -> Result<(), XBibleError> {
    set_state(tasks, tid, StoreTaskState::Running, |p| {
        p.message = format!("Installing {} from {}…", id, source.display_name());
    });

    // Phase 1: Source-agnostic fetch & normalize (runs on shared Tokio runtime inside source).
    let sink = StoreProgressBridge {
        tasks: tasks.clone(),
        task_id: tid.to_string(),
        cancel: cancel.clone(),
        label: id.to_string(),
    };

    let bundle: GenericInstalledBible = source
        .fetch_for_install(id, &sink)
        .map_err(|msg: String| XBibleError::Generic { error_message: msg })?;

    // Phase 2: Chunked SQLite persistence on DbWriter (never holds DB lock during network I/O).
    let total_chapters: i32 = bundle
        .chapters_by_book
        .iter()
        .map(|bc| bc.chapters.len() as i32)
        .sum();
    let id_owned: String = id.to_string();

    writer.submit("begin_install", cancel.clone(), {
        let id: String = id_owned.clone();
        move |conn| install::begin_install(conn, &id)
    })?;

    let mut chapters_done: i32 = 0;
    let mut verses_done: i32 = 0;

    for (book, book_chapters) in bundle.books.iter().zip(bundle.chapters_by_book.iter()) {
        let book_for_job = book.clone();
        let chapters_for_job = book_chapters.clone();
        let id_for_job: String = id_owned.clone();

        writer.submit("persist_book_chapters", cancel.clone(), move |conn| {
            install::persist_book_chapters(
                conn,
                &id_for_job,
                &book_for_job,
                chapters_for_job.chapters.as_slice(),
            )
        })?;

        chapters_done += book_chapters.chapters.len() as i32;
        verses_done += book_chapters
            .chapters
            .iter()
            .map(|c| c.verses.len() as i32)
            .sum::<i32>();

        let fraction: f64 = 0.8 + (chapters_done as f64 / total_chapters.max(1) as f64) * 0.2;
        update_progress(
            tasks,
            tid,
            InstallProgress {
                fraction,
                chapters_done,
                chapters_total: total_chapters,
                current_book_name: book.name.clone(),
                current_book_id: book.id.clone(),
                current_chapter: book_chapters
                    .chapters
                    .last()
                    .map(|c| c.chapter_number)
                    .unwrap_or(0),
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
        let id: String = id_owned;
        move |conn| install::mark_installed(conn, &id)
    })?;

    info!(
        "installed {} from {} ({} chapters, {} verses)",
        id,
        source.source_id(),
        chapters_done,
        verses_done
    );
    Ok(())
}

// ── Internal Engine Hooks ─────────────────────────────────────────────────────

impl StoreApi {
    /// Registers an additional `BibleSource` in the feature's `SourceRegistry`.
    pub fn register_source(&self, source: Arc<dyn BibleSource>) {
        self.registry.register(source);
    }

    pub(crate) fn attach_connection(&self, conn: Arc<Mutex<Connection>>, writer: DbWriter) {
        *write_or_recover(&self.conn) = Some(conn);
        *write_or_recover(&self.writer) = Some(writer);
    }

    pub(crate) fn cancel_all_tasks(&self) {
        for t in lock_or_recover(&self.tasks).values() {
            t.cancel.store(true, Ordering::Relaxed);
        }
    }
}
