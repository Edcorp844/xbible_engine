//! Top-level crate root for the XBible engine (`src/lib.rs`).
//!
//! Organizes shared database infrastructure (`db`), feature-specific engines
//! (`engines`), feature modules (`features`), runtime support (`runtime`), and
//! data providers (`sources`), while exposing UniFFI scaffolding and the
//! `XBibleEngine` facade to foreign-language bindings (Kotlin, Swift, etc.).

#![allow(unknown_lints)]
#![allow(unsafe_attr_outside_unsafe)]

uniffi::setup_scaffolding!();

#[macro_use]
extern crate log;

use std::sync::{Arc, Mutex, RwLock};

use crate::{
    db::connection::Db,
    db::connection_state::{Closed, Open},
    db::writer::DbWriter,
    engines::audio_engine::engine::AudioEngine,
    error::XBibleError,
    features::store::store_api::StoreApi,
};

// ── Top-Level Modules ────────────────────────────────────────────────────────

pub mod db;
pub mod engines;
pub mod error;
pub mod features;
pub mod runtime;
pub mod sources;

// ── Platform Logging ─────────────────────────────────────────────────────────

/// Initializes platform-appropriate logging once across Android, desktop/Linux,
/// iOS, and macOS targets.
pub(crate) fn initialize_logging() {
    #[cfg(target_os = "android")]
    {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Debug)
                .with_tag("XBible Engine"),
        );
    }

    #[cfg(not(target_os = "android"))]
    {
        let env_init = env_logger::builder()
            .filter_level(log::LevelFilter::Debug)
            .is_test(true)
            .try_init();

        #[cfg(any(target_os = "ios", target_os = "macos"))]
        if env_init.is_err() {
            let _ = oslog::OsLogger::new("com.xbible.engine")
                .level_filter(log::LevelFilter::Debug)
                .init();
        }
    }

    log::info!(
        "Initialized logging for XBibleEngine on {}",
        std::env::consts::OS
    );
}

/// Rust-side logging initialization entrypoint.
///
/// Foreign-language consumers receive logging setup automatically when
/// constructing `XBibleEngine`, or explicitly via the UniFFI-exported
/// `init_logging()` function.
pub fn init_rust_logging() {
    initialize_logging();
}

/// UniFFI-exported logging initialization entrypoint.
///
/// Kept as a free function so generated bindings retain the expected exported
/// symbol and checksum while `XBibleEngine::new()` also ensures initialization.
#[uniffi::export]
pub fn init_logging() {
    initialize_logging();
}

// ── Low-Level C / Native Hooks ───────────────────────────────────────────────

/// Required when `libbz2` is built with `BZ_NO_STDIO` (such as on iOS).
///
/// Avoids stdio calls inside the `bzip2` error path before aborting.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bz_internal_error(errcode: std::os::raw::c_int) {
    error!("bzip2 internal error: {errcode}");
    std::process::abort();
}

// ── Top-Level UniFFI Engine Facade ───────────────────────────────────────────

/// Top-level API entry object exposed across the UniFFI boundary.
///
/// Owns the database handle and single-threaded writer across idempotent
/// reopens (`set_db_path`) and exposes feature handles (`audio_engine`,
/// `store`) to Kotlin (Android) and Swift (iOS / macOS).
#[allow(dead_code)]
#[derive(Debug, uniffi::Object)]
pub struct XBibleEngine {
    next_task_id: Arc<Mutex<u64>>,
    db: RwLock<Option<Db<Open>>>,
    writer: RwLock<Option<DbWriter>>,
    audio_engine: Arc<AudioEngine>,
    store: Arc<StoreApi>,
}

impl Default for XBibleEngine {
    fn default() -> Self {
        initialize_logging();
        Self {
            next_task_id: Arc::new(Mutex::new(1)),
            db: RwLock::new(None),
            writer: RwLock::new(None),
            audio_engine: AudioEngine::new(),
            store: StoreApi::new(),
        }
    }
}

#[uniffi::export]
impl XBibleEngine {
    /// Constructs a new `XBibleEngine` instance and initializes logging.
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        initialize_logging();
        Arc::new(Self {
            next_task_id: Arc::new(Mutex::new(1)),
            db: RwLock::new(None),
            writer: RwLock::new(None),
            audio_engine: AudioEngine::new(),
            store: StoreApi::new(),
        })
    }

    /// Opens or reopens the SQLite database at `path`, runs migrations,
    /// spawns a fresh single-threaded `DbWriter`, and reattaches sub-engines.
    ///
    /// Safe to call repeatedly from Kotlin/Swift across lifecycle/path changes:
    ///   1. cancels running store tasks holding clones of the old writer
    ///   2. shuts down and drops the old `DbWriter`
    ///   3. drops the old `Db<Open>` to close the old file handle
    ///   4. opens the database at `path` and runs migrations
    ///   5. spawns a new `DbWriter` and reattaches it to `store`
    pub fn set_db_path(&self, path: String) -> Result<(), XBibleError> {
        // 1. Cancel in-flight tasks before tearing down the writer.
        self.store.cancel_all_tasks();

        // 2. Shut down and drop any existing writer handle.
        if let Some(old_writer) = self
            .writer
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .take()
        {
            old_writer.shutdown();
        }

        // 3. Drop the old `Db<Open>` so its file handle closes cleanly.
        *self.db.write().unwrap_or_else(|p| p.into_inner()) = None;

        // 4. Open the database at `path` (runs migrations).
        let opened: Db<Open> = Db::<Closed>::open(&path)?;
        let shared = opened.share();

        // 5. Spawn a new writer and attach both to `store`.
        let new_writer: DbWriter = DbWriter::spawn(shared.clone());
        self.store.attach_connection(shared, new_writer.clone());

        *self.writer.write().unwrap_or_else(|p| p.into_inner()) = Some(new_writer);
        *self.db.write().unwrap_or_else(|p| p.into_inner()) = Some(opened);
        Ok(())
    }

    /// Returns the audio engine handle.
    pub fn audio_engine(&self) -> Arc<AudioEngine> {
        self.audio_engine.clone()
    }

    /// Returns the store feature handle (`store`).
    pub fn store(&self) -> Arc<StoreApi> {
        self.store.clone()
    }
}
