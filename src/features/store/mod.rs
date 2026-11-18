//! Unified Store: catalog presentation, categorization, filtering, and
//! source-routed installation into the general reader format (`src/features/store/store.rs`).

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;

use log::{error, info};
use rusqlite::Connection;

use crate::db::catalog;
use crate::db::install;
use crate::db::writer::DbWriter;
use crate::error::XBibleError;
use crate::sources::models::GenericInstalledBible;
use crate::sources::{BibleSource, InstallProgressSink, SourceRegistry};

// ── UniFFI Wire Records & Task Types ──────────────────────────────────────────

#[derive(Debug, Clone, uniffi::Record)]
pub struct LanguageFacet {
    pub code: String,
    pub name: String,
    pub english_name: String,
    pub translation_count: i32,
    pub commentary_count: i32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct StoreItem {
    pub id: String,
    pub source_id: String,
    pub kind: String,
    pub name: String,
    pub english_name: String,
    pub short_name: String,
    pub language: String,
    pub language_name: Option<String>,
    pub description: String,
    pub badge: Option<String>,
    pub installed: bool,
    pub has_strongs: bool,
    pub has_audio: bool,
    pub book_count: i32,
    pub chapter_count: i32,
    pub verse_count: i32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct StoreShelf {
    pub title: String,
    pub subtitle: Option<String>,
    pub items: Vec<StoreItem>,
}

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

// ── Categories / Shelves ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreCategory {
    Featured,
    PublicDomain,
    Commentaries,
    RelationshipModules,
    CrossReferences,
    WordStudy,
    Datasets,
    AllResources,
}

impl StoreCategory {
    pub const ALL: &'static [StoreCategory] = &[
        Self::Featured,
        Self::PublicDomain,
        Self::Commentaries,
        Self::RelationshipModules,
        Self::CrossReferences,
        Self::WordStudy,
        Self::Datasets,
        Self::AllResources,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Featured => "Featured",
            Self::PublicDomain => "Public Domain",
            Self::Commentaries => "Commentaries",
            Self::RelationshipModules => "Relationship Modules",
            Self::CrossReferences => "Cross-References",
            Self::WordStudy => "Word Study & Lexicons",
            Self::Datasets => "Study Datasets",
            Self::AllResources => "All Resources",
        }
    }

    pub fn subtitle(self) -> Option<&'static str> {
        match self {
            Self::Featured => Some("Curated translations, commentaries, and study modules"),
            Self::PublicDomain => Some("Freely distributable classic texts"),
            Self::Commentaries => Some("Verse-by-verse notes and exposition"),
            Self::RelationshipModules => Some("People, places, events, and entity graphs"),
            Self::CrossReferences => Some("Parallel passages and scripture links"),
            Self::WordStudy => Some("Strong's numbers, interlinears, and lexicons"),
            Self::Datasets => Some("Structured reference and lexical datasets"),
            Self::AllResources => None,
        }
    }

    pub fn from_label(label: &str) -> Option<Self> {
        let normalized = label.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "featured" => Some(Self::Featured),
            "public domain" | "public_domain" | "public-domain" => Some(Self::PublicDomain),
            "commentaries" | "commentary" => Some(Self::Commentaries),
            "relationship modules"
            | "relationship_modules"
            | "relationships"
            | "theographic"
            | "entities" => Some(Self::RelationshipModules),
            "cross-references" | "cross_references" | "cross references" | "xrefs" => {
                Some(Self::CrossReferences)
            }
            "word study" | "word_study" | "word study & lexicons" | "lexicons" => {
                Some(Self::WordStudy)
            }
            "datasets" | "dataset" | "study datasets" => Some(Self::Datasets),
            "all resources" | "all" => Some(Self::AllResources),
            _ => Self::ALL
                .iter()
                .copied()
                .find(|c: &Self| c.title().eq_ignore_ascii_case(label.trim())),
        }
    }
}

// ── Internal Catalog & Task State ─────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
struct CatalogState {
    items: Vec<StoreItem>,
    languages: Vec<LanguageFacet>,
}

#[derive(Debug)]
struct TaskCounter(u64);

#[allow(dead_code)]
#[derive(Debug)]
struct TaskEntry {
    target: String,
    status: StoreTaskStatus,
    cancel: Arc<AtomicBool>,
}

struct StoreProgressBridge {
    tasks: Arc<Mutex<HashMap<String, TaskEntry>>>,
    task_id: String,
    cancel: Arc<AtomicBool>,
    label: String,
}

impl InstallProgressSink for StoreProgressBridge {
    fn on_progress(&self, stage: String, current: u32, total: u32, message: Option<String>) {
        let fraction: f64 = match stage.as_str() {
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
            "normalize" => format!("Formatting {}…", self.label),
            _ => format!("Installing {}…", self.label),
        });

        let (chapters_done, chapters_total) = if stage == "download" {
            (current as i32, total as i32)
        } else {
            (0, 0)
        };

        update_progress(
            &self.tasks,
            &self.task_id,
            InstallProgress {
                fraction,
                chapters_done,
                chapters_total,
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

#[derive(uniffi::Object)]
pub struct Store {
    conn: Arc<RwLock<Option<Arc<Mutex<Connection>>>>>,
    writer: RwLock<Option<DbWriter>>,
    catalog: RwLock<CatalogState>,
    registry: Arc<SourceRegistry>,
    tasks: Arc<Mutex<HashMap<String, TaskEntry>>>,
    next_id: Arc<Mutex<TaskCounter>>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
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

fn alloc_task_id(next_id: &Arc<Mutex<TaskCounter>>, prefix: &str, target: &str) -> String {
    let mut counter = lock_or_recover(next_id);
    let task_id: String = format!("{prefix}_{target}_{}", counter.0);
    counter.0 += 1;
    task_id
}

fn register_task(
    tasks: &Arc<Mutex<HashMap<String, TaskEntry>>>,
    task_id: &str,
    target: &str,
    message: &str,
    cancel: &Arc<AtomicBool>,
) {
    let entry: TaskEntry = TaskEntry {
        target: target.to_string(),
        status: StoreTaskStatus {
            task_id: task_id.to_string(),
            state: StoreTaskState::Queued,
            progress: empty_progress(format!("{message} {target}")),
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

// ── Catalog Categorization & Installed Overlay ────────────────────────────────

impl CatalogState {
    fn from_items(items: Vec<StoreItem>) -> Self {
        let languages: Vec<LanguageFacet> = Self::build_facets(&items);
        Self { items, languages }
    }

    fn build_facets(items: &[StoreItem]) -> Vec<LanguageFacet> {
        let mut map: HashMap<String, (String, String, i32, i32)> = HashMap::new();
        for item in items {
            let lang_code = item.language.clone();
            let display_name = item
                .language_name
                .clone()
                .unwrap_or_else(|| lang_code.clone());
            let entry =
                map.entry(lang_code.clone())
                    .or_insert((display_name.clone(), display_name, 0, 0));
            if item.kind == "commentary" {
                entry.3 += 1;
            } else if item.kind == "translation" {
                entry.2 += 1;
            }
        }
        let mut out: Vec<LanguageFacet> = map
            .into_iter()
            .map(|(code, (name, eng, tc, cc))| LanguageFacet {
                code,
                name,
                english_name: eng,
                translation_count: tc,
                commentary_count: cc,
            })
            .collect();
        out.sort_by(|a, b| a.english_name.cmp(&b.english_name));
        out
    }

    fn category_items(&self, category: StoreCategory, language: Option<&str>) -> Vec<StoreItem> {
        let matches_lang = |i: &StoreItem| language.map(|l| i.language == l).unwrap_or(true);

        match category {
            StoreCategory::Featured => self
                .items
                .iter()
                .filter(|i| matches_lang(i))
                .filter(|i| {
                    matches!(
                        i.kind.as_str(),
                        "translation" | "commentary" | "entity_set" | "dataset"
                    )
                })
                .take(12)
                .cloned()
                .collect(),

            StoreCategory::PublicDomain => self
                .items
                .iter()
                .filter(|i| matches_lang(i))
                .filter(|i| {
                    i.kind == "translation"
                        && (i
                            .badge
                            .as_deref()
                            .map(|b| b.to_ascii_lowercase().contains("public domain"))
                            .unwrap_or(false)
                            || matches!(
                                i.id.to_ascii_lowercase().as_str(),
                                "eng_kjv"
                                    | "eng_web"
                                    | "eng_asv"
                                    | "eng_kja"
                                    | "web"
                                    | "asv"
                                    | "kjv"
                            ))
                })
                .cloned()
                .collect(),

            StoreCategory::Commentaries => self
                .items
                .iter()
                .filter(|i| matches_lang(i) && i.kind == "commentary")
                .cloned()
                .collect(),

            StoreCategory::RelationshipModules => self
                .items
                .iter()
                .filter(|i| matches_lang(i))
                .filter(|i| {
                    i.kind == "entity_set"
                        || i.kind == "relationship_module"
                        || i.id.starts_with("theographic:")
                        || i.badge
                            .as_deref()
                            .map(|b| {
                                let bl = b.to_ascii_lowercase();
                                bl.contains("people")
                                    || bl.contains("places")
                                    || bl.contains("events")
                                    || bl.contains("groups")
                                    || bl.contains("relationship")
                            })
                            .unwrap_or(false)
                })
                .cloned()
                .collect(),

            StoreCategory::CrossReferences => self
                .items
                .iter()
                .filter(|i| matches_lang(i))
                .filter(|i| {
                    i.kind == "cross_reference"
                        || i.id.to_ascii_lowercase().contains("xref")
                        || i.id.to_ascii_lowercase().contains("cross-ref")
                        || i.name.to_ascii_lowercase().contains("cross-reference")
                        || i.name.to_ascii_lowercase().contains("cross reference")
                })
                .cloned()
                .collect(),

            StoreCategory::WordStudy => self
                .items
                .iter()
                .filter(|i| matches_lang(i))
                .filter(|i| {
                    i.has_strongs
                        || i.id.to_ascii_lowercase().contains("lexicon")
                        || i.id.to_ascii_lowercase().contains("interlinear")
                        || i.id == "BSB"
                })
                .cloned()
                .collect(),

            StoreCategory::Datasets => self
                .items
                .iter()
                .filter(|i| matches_lang(i) && i.kind == "dataset")
                .cloned()
                .collect(),

            StoreCategory::AllResources => {
                let mut out: Vec<StoreItem> = self
                    .items
                    .iter()
                    .filter(|i| matches_lang(i))
                    .cloned()
                    .collect();
                out.sort_by(|a, b| a.name.cmp(&b.name));
                out
            }
        }
    }
}

fn overlay_installed_flags(
    conn: &Connection,
    tasks: &Arc<Mutex<HashMap<String, TaskEntry>>>,
    items: &mut [StoreItem],
) {
    let running: bool = lock_or_recover(tasks).values().any(|t: &TaskEntry| {
        matches!(
            t.status.state,
            StoreTaskState::Queued | StoreTaskState::Running
        )
    });
    if running {
        return;
    }

    for item in items.iter_mut() {
        let installed: i32 = conn
            .query_row(
                "SELECT installed FROM translations WHERE id = ?1",
                rusqlite::params![item.id],
                |r| r.get(0),
            )
            .unwrap_or(if item.installed { 1 } else { 0 });
        item.installed = installed != 0;
    }
}

// ── Public Store API (UniFFI Object) ──────────────────────────────────────────

#[uniffi::export]
impl Store {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            conn: Arc::new(RwLock::new(None)),
            writer: RwLock::new(None),
            catalog: RwLock::new(CatalogState::default()),
            registry: Arc::new(SourceRegistry::new()),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(Mutex::new(TaskCounter(0))),
        })
    }

    /// Asks each registered source for its `StoreItem`s and updates the store catalog.
    pub fn refresh(&self) -> Result<(), XBibleError> {
        let sources: Vec<Arc<dyn BibleSource>> = self.registry.all();
        let mut collected: Vec<StoreItem> = Vec::new();
        let mut any_source_ok: bool = false;
        let mut last_err: Option<String> = None;

        for source in sources {
            let sid: String = source.source_id().to_string();
            match source.list_items() {
                Ok(mut items) => {
                    any_source_ok = true;
                    for item in &mut items {
                        if item.source_id.is_empty() {
                            item.source_id = sid.clone();
                        }
                    }
                    collected.extend(items);
                }
                Err(err) => {
                    error!("source {sid} list_items failed: {err}");
                    last_err = Some(format!("{sid}: {err}"));
                }
            }
        }

        if any_source_ok {
            let mut state = CatalogState::from_items(collected);
            if let Some(conn_arc) = read_or_recover(&self.conn).clone() {
                let conn = lock_or_recover(&conn_arc);
                overlay_installed_flags(&conn, &self.tasks, &mut state.items);
            }
            *write_or_recover(&self.catalog) = state;
            info!("store catalog refreshed from registered sources");
            return Ok(());
        }

        // Offline fallback to cached translations in SQLite if all sources failed.
        let conn_arc = read_or_recover(&self.conn)
            .clone()
            .ok_or_else(|| XBibleError::Generic {
                error_message: "source refresh failed and no local database is open".into(),
            })?;
        let conn = lock_or_recover(&conn_arc);
        let cached = catalog::load_translations(&conn).map_err(|e| XBibleError::Generic {
            error_message: format!("{e:?}"),
        })?;
        if cached.is_empty() {
            let detail = last_err.unwrap_or_else(|| "no sources registered".to_string());
            return Err(XBibleError::Generic {
                error_message: format!("source refresh failed ({detail}) and local cache is empty"),
            });
        }

        let fallback_items: Vec<StoreItem> = cached
            .into_iter()
            .map(|t| StoreItem {
                id: t.id.clone(),
                source_id: t.source_id.clone(),
                kind: "translation".to_string(),
                name: t.name.clone(),
                english_name: t.english_name.clone().unwrap_or_else(|| t.name.clone()),
                short_name: t.short_name.clone().unwrap_or_else(|| t.id.clone()),
                language: t.language_code.clone(),
                language_name: t.language_name.clone(),
                description: format!(
                    "{} • {} books • {} verses",
                    t.language_english_name
                        .clone()
                        .unwrap_or_else(|| t.language_code.clone()),
                    t.book_count.unwrap_or(0),
                    t.verse_count.unwrap_or(0),
                ),
                badge: None,
                installed: false,
                has_strongs: t.has_strongs.unwrap_or(false),
                has_audio: t.has_audio,
                book_count: t.book_count.unwrap_or(0),
                chapter_count: t.chapter_count.unwrap_or(0),
                verse_count: t.verse_count.unwrap_or(0),
            })
            .collect();

        let mut state = CatalogState::from_items(fallback_items);
        overlay_installed_flags(&conn, &self.tasks, &mut state.items);
        *write_or_recover(&self.catalog) = state;
        info!("store catalog loaded from local cache");
        Ok(())
    }

    pub fn home_feed(&self, language: Option<String>) -> Vec<StoreShelf> {
        let state = read_or_recover(&self.catalog);
        let mut shelves: Vec<StoreShelf> = StoreCategory::ALL
            .iter()
            .copied()
            .map(|cat| {
                let items = state.category_items(cat, language.as_deref());
                let subtitle = cat.subtitle().map(str::to_string).or_else(|| {
                    if cat == StoreCategory::AllResources {
                        Some(format!("{} items", items.len()))
                    } else {
                        None
                    }
                });
                StoreShelf {
                    title: cat.title().to_string(),
                    subtitle,
                    items,
                }
            })
            .filter(|s| !s.items.is_empty())
            .collect();

        drop(state);
        if let Some(conn_arc) = read_or_recover(&self.conn).clone() {
            let conn = lock_or_recover(&conn_arc);
            for shelf in &mut shelves {
                overlay_installed_flags(&conn, &self.tasks, &mut shelf.items);
            }
        }
        shelves
    }

    pub fn items_by_category(&self, category: String, language: Option<String>) -> Vec<StoreItem> {
        let Some(cat) = StoreCategory::from_label(&category) else {
            return Vec::new();
        };
        let mut items = read_or_recover(&self.catalog).category_items(cat, language.as_deref());
        if let Some(conn_arc) = read_or_recover(&self.conn).clone() {
            let conn = lock_or_recover(&conn_arc);
            overlay_installed_flags(&conn, &self.tasks, &mut items);
        }
        items
    }

    pub fn get_shelf(
        &self,
        shelf_id: String,
        language: Option<String>,
        offset: u32,
        limit: u32,
    ) -> Vec<StoreItem> {
        let Some(cat) = StoreCategory::from_label(&shelf_id) else {
            return Vec::new();
        };
        let mut items: Vec<StoreItem> = read_or_recover(&self.catalog)
            .category_items(cat, language.as_deref())
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect();

        if let Some(conn_arc) = read_or_recover(&self.conn).clone() {
            let conn = lock_or_recover(&conn_arc);
            overlay_installed_flags(&conn, &self.tasks, &mut items);
        }
        items
    }

    pub fn shelf_total(&self, shelf_id: String, language: Option<String>) -> u32 {
        let Some(cat) = StoreCategory::from_label(&shelf_id) else {
            return 0;
        };
        read_or_recover(&self.catalog)
            .category_items(cat, language.as_deref())
            .len() as u32
    }

    pub fn search(&self, query: String) -> Vec<StoreItem> {
        let q: String = query.trim().to_lowercase();
        let state = read_or_recover(&self.catalog);
        let mut items: Vec<StoreItem> = if q.is_empty() {
            state.items.clone()
        } else {
            state
                .items
                .iter()
                .filter(|item| {
                    item.id.to_lowercase().contains(&q)
                        || item.name.to_lowercase().contains(&q)
                        || item.english_name.to_lowercase().contains(&q)
                        || item.short_name.to_lowercase().contains(&q)
                        || item.kind.to_lowercase().contains(&q)
                        || item
                            .badge
                            .as_deref()
                            .map(|b| b.to_lowercase().contains(&q))
                            .unwrap_or(false)
                })
                .cloned()
                .collect()
        };
        items.sort_by(|a, b| a.name.cmp(&b.name));
        drop(state);

        if let Some(conn_arc) = read_or_recover(&self.conn).clone() {
            let conn = lock_or_recover(&conn_arc);
            overlay_installed_flags(&conn, &self.tasks, &mut items);
        }
        items
    }

    pub fn languages(&self) -> Vec<LanguageFacet> {
        read_or_recover(&self.catalog).languages.clone()
    }

    pub fn item(&self, id: String, kind: String) -> Option<StoreItem> {
        let mut found: StoreItem = read_or_recover(&self.catalog)
            .items
            .iter()
            .find(|i| i.id == id && (kind.is_empty() || i.kind == kind))
            .cloned()?;

        if let Some(conn_arc) = read_or_recover(&self.conn).clone() {
            let conn = lock_or_recover(&conn_arc);
            overlay_installed_flags(&conn, &self.tasks, std::slice::from_mut(&mut found));
        }
        Some(found)
    }

    /// Single source-routed install entry point: hands `item` to `item.source_id`,
    /// which downloads and formats it into the general reader representation.
    pub fn install(&self, item: StoreItem) -> Result<String, XBibleError> {
        let source: Arc<dyn BibleSource> =
            self.registry
                .get(&item.source_id)
                .ok_or_else(|| XBibleError::Generic {
                    error_message: format!("unknown source: {}", item.source_id),
                })?;

        let writer: DbWriter = read_or_recover(&self.writer)
            .clone()
            .ok_or_else(not_open_err)?;

        let task_id: String = alloc_task_id(&self.next_id, "inst", &item.id);
        let cancel: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        register_task(&self.tasks, &task_id, &item.id, "Queued", &cancel);

        let tid: String = task_id.clone();
        let tasks = self.tasks.clone();
        let success_message: String = format!("{} installed", item.id);

        run_task(
            &self.tasks,
            &task_id,
            "xbible-inst",
            success_message,
            "Install failed",
            move || {
                set_state(&tasks, &tid, StoreTaskState::Running, |p| {
                    p.fraction = 0.05;
                    p.message = format!("Installing {} via {}…", item.id, source.display_name());
                });

                let sink = StoreProgressBridge {
                    tasks: tasks.clone(),
                    task_id: tid.clone(),
                    cancel: cancel.clone(),
                    label: item.id.clone(),
                };

                let bundle: GenericInstalledBible = source
                    .install_item(&item, &sink)
                    .map_err(|msg: String| XBibleError::Generic { error_message: msg })?;

                // ── Put the snippet right here (before `begin_install`) ──────────────────
                let cat = bundle.catalog.clone();
                let id_for_catalog: String = item.id.clone();
                writer.submit("ensure_catalog_entry", cancel.clone(), move |conn| {
                    let dir = match cat.text_direction {
                        crate::sources::models::TextDirection::RightToLeft => "rtl",
                        crate::sources::models::TextDirection::LeftToRight => "ltr",
                    };
                    let _ = conn.execute(
                        "INSERT OR IGNORE INTO translations (
                            id, name, english_name, short_name, language,
                            language_name, language_english_name, text_direction,
                            number_of_books, total_number_of_chapters, total_number_of_verses,
                            installed, has_strongs, source_id, language_code,
                            book_count, chapter_count, verse_count, has_text, has_audio
                         ) VALUES (
                            ?1, ?2, ?3, ?4, ?5,
                            ?6, ?7, ?8,
                            ?9, ?10, ?11,
                            0, ?12, ?13, ?5,
                            ?9, ?10, ?11, ?14, ?15
                         )",
                        rusqlite::params![
                            id_for_catalog,
                            cat.name,
                            cat.english_name.unwrap_or_default(),
                            cat.short_name.unwrap_or_default(),
                            cat.language_code,
                            cat.language_name,
                            cat.language_english_name,
                            dir,
                            cat.book_count.unwrap_or(0),
                            cat.chapter_count.unwrap_or(0),
                            cat.verse_count.unwrap_or(0),
                            cat.has_strongs.unwrap_or(false) as i32,
                            cat.source_id,
                            cat.has_audio as i32,
                            cat.has_text as i32,
                        ],
                    );
                    Ok(())
                })?;
                // ─────────────────────────────────────────────────────────────────────────

                let total_chapters: i32 = bundle
                    .chapters_by_book
                    .iter()
                    .map(|bc| bc.chapters.len() as i32)
                    .sum();
                let id_owned: String = item.id.clone();

                writer.submit("begin_install", cancel.clone(), {
                    let id: String = id_owned.clone();
                    move |conn| install::begin_install(conn, &id)
                })?;

                let mut chapters_done: i32 = 0;
                let mut verses_done: i32 = 0;

                for (book, book_chapters) in bundle.books.iter().zip(bundle.chapters_by_book.iter())
                {
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

                    let fraction: f64 =
                        0.8 + (chapters_done as f64 / total_chapters.max(1) as f64) * 0.2;
                    update_progress(
                        &tasks,
                        &tid,
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
                    item.id,
                    source.source_id(),
                    chapters_done,
                    verses_done
                );
                Ok(())
            },
        );

        Ok(task_id)
    }

    pub fn uninstall_translation(&self, translation_id: String) -> Result<String, XBibleError> {
        let writer: DbWriter = read_or_recover(&self.writer)
            .clone()
            .ok_or_else(not_open_err)?;

        let task_id: String = alloc_task_id(&self.next_id, "uninst", &translation_id);
        let cancel: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        register_task(&self.tasks, &task_id, &translation_id, "Queued", &cancel);

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

// ── Internal Engine Hooks ─────────────────────────────────────────────────────

impl Store {
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
