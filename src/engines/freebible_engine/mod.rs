// freebible_engine/mod.rs

pub mod client;
pub mod error;
pub mod install;
pub mod reader;
pub mod store;
pub mod store_api;
pub mod utils;
pub mod values;

use std::sync::{Arc, RwLock};

use crate::db::connection::Db;
use crate::db::connection_state::Closed;
use crate::engines::freebible_engine::error::FreeBibleError;
use crate::engines::freebible_engine::store_api::FreeBibleStore;
use crate::{db::connection_state::Open, engines::freebible_engine::reader::FreeBibleReader};

/// Top-level FreeBible engine.
///
/// Owns the SQLite handle. On the Rust side this is a `Db<S>` type-state
/// so `query`/`write` methods only exist once the DB is open. On the
/// FFI side we expose a single concrete `FreeBibleEngine` — the type
/// parameter only exists to catch mistakes in Rust code before they
/// hit the FFI boundary.
#[derive(Debug, uniffi::Object)]
pub struct FreeBibleEngine {
    /// `None` until `set_db_path` is called. Once open, the Arc is
    /// shared with both sub-engines.
    db: RwLock<Option<Db<Open>>>,

    store: Arc<FreeBibleStore>,
    reader: Arc<FreeBibleReader>,
}

// ── Public FFI surface ────────────────────────────────────────

#[uniffi::export]
impl FreeBibleEngine {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            db: RwLock::new(None),
            store: FreeBibleStore::new(),
            reader: FreeBibleReader::new(),
        })
    }

    /// Open the database at `path` and share the connection with both
    /// sub-engines. Idempotent: calling twice reopens to the new path.
    pub fn set_db_path(&self, path: String) -> Result<(), FreeBibleError> {
        // Close any existing DB first.
        {
            let mut guard = self.db.write().unwrap_or_else(|p| p.into_inner());
            *guard = None;
        }

        let opened = Db::<Closed>::open(&path)?;

        let shared = opened.share();
        self.store.attach_connection(shared.clone());
        self.reader.attach_connection(shared);

        let mut guard = self.db.write().unwrap_or_else(|p| p.into_inner());
        *guard = Some(opened);
        Ok(())
    }

    /// Store sub-engine: catalog, shelves, search, installs.
    pub fn store(&self) -> Arc<FreeBibleStore> {
        self.store.clone()
    }

    /// Reader sub-engine: chapters, books, entities, verse lookups.
    pub fn reader(&self) -> Arc<FreeBibleReader> {
        self.reader.clone()
    }
}

// ── Re-exports for Kotlin / UniFFI ────────────────────────────

pub use client::{
    BibleClient, CommentaryProfile, EntitySummary, FreeBibleBook, FreeBibleChapter,
    FreeBibleChapterItem, FreeBibleCommentary, FreeBibleCommentaryChapter, FreeBibleComplete,
    FreeBibleCompleteBook, FreeBibleCompleteChapter, FreeBibleDataset, FreeBibleFootnote,
    FreeBibleLexiconEntry, FreeBibleTranslation, FreeBibleVerse,
};
pub use reader::{
    EntityRef, EventDetail, GroupDetail, InstalledTranslation, PersonDetail, PlaceDetail,
    ReaderBlock, ReaderChapter, ReaderVerse, ReaderWord, VerseRange,
};
pub use store::{LanguageFacet, Store, StoreItem, StoreShelf};
pub use utils::{FreeBibleTaskState, FreeBibleTaskStatus, InstallProgress};
