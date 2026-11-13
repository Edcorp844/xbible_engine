// freebible_engine/mod.rs

pub mod catalog;
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
use crate::db::connection_state::Open;
use crate::db::writer::DbWriter;
use crate::engines::freebible_engine::error::FreeBibleError;
use crate::engines::freebible_engine::reader::FreeBibleReader;
use crate::engines::freebible_engine::store_api::FreeBibleStore;

/// Top-level FreeBible engine.
///
/// Owns the SQLite handle and the single writer thread. On the Rust
/// side `Db<S>` is a type-state so query/write methods only exist
/// once the DB is open. On the FFI side only `FreeBibleEngine` is
/// exposed; the type parameter catches mistakes before the boundary.
#[derive(Debug, uniffi::Object)]
pub struct FreeBibleEngine {
    /// `None` until `set_db_path` is called. Once open, the
    /// `Arc<Mutex<Connection>>` is shared with both sub-engines
    /// (reads) and with the writer (writes).
    db: RwLock<Option<Db<Open>>>,

    /// `None` until `set_db_path`. Owned here so it can be shut down
    /// before a reopen; the store only holds a clone for submitting
    /// jobs.
    writer: RwLock<Option<DbWriter>>,

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
            writer: RwLock::new(None),
            store: FreeBibleStore::new(),
            reader: FreeBibleReader::new(),
        })
    }

    /// Open the database at `path` and share it with both sub-engines
    /// and the writer. Idempotent: calling twice reopens to the new
    /// path.
    ///
    /// Order matters:
    ///   1. cancel running tasks (they hold clones of the old writer)
    ///   2. shut the old writer down (so it stops touching the old DB)
    ///   3. drop the old `Db<Open>` (so the old file handle closes)
    ///   4. open the new DB (which runs migrations)
    ///   5. spawn the new writer and reattach both sub-engines
    pub fn set_db_path(&self, path: String) -> Result<(), FreeBibleError> {
        // 1. Cancel running tasks. They will observe the flag and
        //    stop submitting jobs before we tear the writer down.
        self.store.cancel_all_tasks();

        // 2. Shut the old writer down and drop our handle to it. Any
        //    task still holding a clone will get "writer gone" on its
        //    next submit and fail cleanly.
        if let Some(writer) = self
            .writer
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            writer.shutdown();
        }

        // 3. Drop the old Db<Open> so its file handle closes.
        *self.db.write().unwrap_or_else(|p| p.into_inner()) = None;

        // 4. Open the new database. This runs migrations.
        let opened = Db::<Closed>::open(&path)?;
        let shared = opened.share();

        // 5. Spawn the new writer and attach everything.
        let writer = DbWriter::spawn(shared.clone());
        self.reader.attach_connection(shared.clone());
        self.store.attach_connection(shared, writer.clone());

        *self.writer.write().unwrap_or_else(|p| p.into_inner()) = Some(writer);
        *self.db.write().unwrap_or_else(|p| p.into_inner()) = Some(opened);
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
    CommentaryBlock, CommentaryChapter, EntityRef, EventDetail, GroupDetail, InstalledTranslation,
    PersonDetail, PlaceDetail, ReaderBlock, ReaderChapter, ReaderVerse, ReaderWord, VerseRange,
};

pub use store::{LanguageFacet, Store, StoreItem, StoreShelf};
pub use utils::{FreeBibleTaskState, FreeBibleTaskStatus, InstallProgress};
