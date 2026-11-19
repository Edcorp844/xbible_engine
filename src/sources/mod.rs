//! Generic, source-agnostic ingestion models, source contracts, and registry (`src/sources/mod.rs`).
//!
//! Each provider (`helloao`, `fcbh`, `api_bible`, `open_bible`) owns its own
//! HTTP client, authentication/endpoints, and raw JSON deserialization structs
//! internally, wraps its catalog resources (translations, commentaries,
//! datasets, relationship modules, cross-references) as `StoreItem`s on demand,
//! and normalizes installed payloads into the `Generic*` types defined here
//! for persistence and consumption by the Reader.
//!
//! UniFFI & concurrency notes:
//! - Exposed data types derive `uniffi::Record` or `uniffi::Enum` using owned
//!   types (`String`, `Vec`, `Option`, `bool`, `i32`, `i64`).
//! - Nested chapter collections use `Vec<GenericBookChapters>` instead of maps
//!   to keep cross-language FFI bindings straightforward.
//! - `SourceRegistry` uses `RwLock` so sources can be registered or queried
//!   safely from background/store threads without requiring `&mut self` everywhere.

pub mod helloao;
pub mod models;

use std::collections::HashMap;
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use rusqlite::Connection;

use crate::db::install::{persist_installed_bible, uninstall_translation};
use crate::error::XBibleError;
use crate::{
    features::store::StoreItem,
    sources::models::{GenericBook, GenericCatalogBible, GenericChapter, GenericInstalledBible},
};

// ═════════════════════════════════════════════════════════════════════════════
// 4. Source Contract, Installer & Registry
// ═════════════════════════════════════════════════════════════════════════════
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum InstallPhase {
    Catalog,
    Download,
    Normalize,
    Install,
    Done,
}

/// Progress and cancellation interface passed to a `BibleSource` during
/// multi-step download or installation work.
#[uniffi::export(callback_interface)]
pub trait InstallProgressSink: Send + Sync {
    /// Reports current progress for a stage (`current` out of `total`).
    fn on_progress(&self, phase: InstallPhase, current: u32, total: u32, message: Option<String>);

    /// Returns true if the caller/store requested cancellation.
    fn is_cancelled(&self) -> bool;
}

// pub trait InstallProgressSink: Send + Sync {
//     /// Reports current progress for a stage (`current` out of `total`).
//     fn on_progress(&self, stage: String, current: u32, total: u32, message: Option<String>);

//     fn is_cancelled(&self) -> bool;
// }

/// Provider abstraction isolating upstream API differences behind `StoreItem`
/// presentation records and normalized `Generic*` reader payloads.
pub trait BibleSource: Send + Sync {
    /// Unique identifier for this source (e.g., `"helloao"`, `"fcbh"`, `"api_bible"`, `"open_bible"`).
    fn source_id(&self) -> &str;

    /// Human-readable name for UI or logging.
    fn display_name(&self) -> &str;

    /// Fetches available Bible catalog entries from this source.
    fn list_catalog(&self) -> Result<Vec<GenericCatalogBible>, String>;

    /// Wraps this source's resources (translations, commentaries, datasets,
    /// relationship modules, cross-references) as `StoreItem`s for the Store.
    fn list_items(&self) -> Result<Vec<StoreItem>, String> {
        let sid: String = self.source_id().to_string();
        let catalog = self.list_catalog()?;
        Ok(catalog
            .into_iter()
            .map(|t| StoreItem {
                id: t.id.clone(),
                source_id: if t.source_id.is_empty() {
                    sid.clone()
                } else {
                    t.source_id.clone()
                },
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
            .collect())
    }

    /// Fetches book metadata for a specific Bible ID within this source.
    fn list_books(&self, bible_id: &str) -> Result<Vec<GenericBook>, String>;

    /// Fetches and normalizes a single chapter on demand.
    fn fetch_chapter(
        &self,
        bible_id: &str,
        book_id: &str,
        chapter_number: i32,
    ) -> Result<GenericChapter, String>;

    /// Downloads, normalizes, and returns a full Bible bundle ready for DB persistence.
    fn fetch_for_install(
        &self,
        bible_id: &str,
        progress: &dyn InstallProgressSink,
    ) -> Result<GenericInstalledBible, String>;

    /// Looks up and installs a `StoreItem` selected in the Store—determining
    /// its internal type/structure within this source and formatting it into
    /// the general `GenericInstalledBible` representation accepted by the reader.
    fn install_item(
        &self,
        item: &StoreItem,
        progress: &dyn InstallProgressSink,
    ) -> Result<GenericInstalledBible, String> {
        self.fetch_for_install(&item.id, progress)
    }
}

/// Source-level installation contract so each provider installs and uninstalls
/// its own items using normalized persistence helpers.
pub trait SourceInstaller: BibleSource {
    /// Fetches and persists a complete Bible bundle into SQLite.
    fn install_bible(
        &self,
        conn: &Connection,
        bible_id: &str,
        progress: &dyn InstallProgressSink,
    ) -> Result<GenericInstalledBible, XBibleError> {
        let bundle: GenericInstalledBible = self
            .fetch_for_install(bible_id, progress)
            .map_err(|msg: String| XBibleError::Generic { error_message: msg })?;
        persist_installed_bible(conn, &bundle)?;
        Ok(bundle)
    }

    /// Looks up, normalizes, and persists a `StoreItem` into SQLite.
    fn install_store_item(
        &self,
        conn: &Connection,
        item: &StoreItem,
        progress: &dyn InstallProgressSink,
    ) -> Result<GenericInstalledBible, XBibleError> {
        let bundle: GenericInstalledBible = self
            .install_item(item, progress)
            .map_err(|msg: String| XBibleError::Generic { error_message: msg })?;
        persist_installed_bible(conn, &bundle)?;
        Ok(bundle)
    }

    /// Removes installed content for `bible_id` and resets its `installed` flag.
    fn uninstall_bible(&self, conn: &Connection, bible_id: &str) -> Result<(), XBibleError> {
        uninstall_translation(conn, bible_id)
    }
}

impl<T: BibleSource + ?Sized> SourceInstaller for T {}

/// Thread-safe registry of configured `BibleSource` implementations.
#[derive(Default)]
pub struct SourceRegistry {
    sources: RwLock<HashMap<String, Arc<dyn BibleSource>>>,
}

impl SourceRegistry {
    /// Creates an empty source registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers or replaces a source keyed by `source.source_id()`.
    pub fn register(&self, source: Arc<dyn BibleSource>) {
        let key: String = source.source_id().to_string();
        if let Ok(mut guard) = self.sources.write() {
            let guard: &mut RwLockWriteGuard<'_, HashMap<String, Arc<dyn BibleSource>>> =
                &mut guard;
            guard.insert(key, source);
        }
    }

    /// Looks up a registered source by `source_id`.
    pub fn get(&self, source_id: &str) -> Option<Arc<dyn BibleSource>> {
        self.sources.read().ok().and_then(
            |guard: RwLockReadGuard<'_, HashMap<String, Arc<dyn BibleSource>>>| {
                guard.get(source_id).cloned()
            },
        )
    }

    /// Returns a snapshot of all registered sources.
    pub fn all(&self) -> Vec<Arc<dyn BibleSource>> {
        self.sources
            .read()
            .ok()
            .map(
                |guard: RwLockReadGuard<'_, HashMap<String, Arc<dyn BibleSource>>>| {
                    guard
                        .values()
                        .cloned()
                        .collect::<Vec<Arc<dyn BibleSource>>>()
                },
            )
            .unwrap_or_default()
    }
}
