// freebible_engine/connection.rs

//! Typed wrapper around `rusqlite::Connection`.
//!
//! `Db<Closed>` has no connection and no query methods.
//! `Db<Open>` holds an `Arc<Mutex<Connection>>` shared with the rest
//! of the engine.
//!
//! The raw `rusqlite::Connection` type is **never** exposed outside
//! this module. All SQL goes through `with_conn` / `with_conn_mut`,
//! both of which only exist on `Db<Open>`.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use crate::db::connection_state::{Closed, ConnectionState, Open};
use crate::db::error::DbError;

// ── The connection handle ───────────────────────────────────────

/// Typed database handle.
///
/// `Db<Closed>` has no connection and no query methods.
/// `Db<Open>` has a live `Arc<Mutex<Connection>>` and exposes
/// `with_conn`, `with_conn_mut`, and `share`.
#[derive(Debug)]
pub struct Db<S: ConnectionState> {
    inner: Option<Arc<Mutex<Connection>>>,
    _state: std::marker::PhantomData<S>,
}

// ── Closed state ────────────────────────────────────────────────

impl Db<Closed> {
    /// Create an unopened handle. No file is touched.
    pub fn new() -> Self {
        Self {
            inner: None,
            _state: std::marker::PhantomData,
        }
    }

    /// Open the SQLite database at `path` and return a `Db<Open>`.
    ///
    /// Returns the shared [`DbError`] — callers convert to their
    /// own engine-specific error type via `From`.
    pub fn open(path: &str) -> Result<Db<Open>, DbError> {
        let conn = crate::db::open(path)?;
        Ok(Db {
            inner: Some(Arc::new(Mutex::new(conn))),
            _state: std::marker::PhantomData,
        })
    }
}

impl Default for Db<Closed> {
    fn default() -> Self {
        Self::new()
    }
}

// ── Open state ──────────────────────────────────────────────────

impl Db<Open> {
    /// Wrap an existing `Arc<Mutex<Connection>>`.
    ///
    /// Used when the engine wants to produce a second handle pointing
    /// at the same physical connection (e.g. one for store, one for
    /// reader). Never construct this from outside the engine.
    pub(crate) fn from_arc(arc: Arc<Mutex<Connection>>) -> Self {
        Self {
            inner: Some(arc),
            _state: std::marker::PhantomData,
        }
    }

    /// Borrow the connection under lock.
    ///
    /// The `&Connection` you get cannot outlive the closure, so no
    /// reference to the raw connection can escape `Db<Open>`.
    ///
    /// ```ignore
    /// let count: i64 = db.with_conn(|conn| {
    ///     conn.query_row("SELECT COUNT(*) FROM books", [], |r| r.get(0))
    ///         .unwrap_or(0)
    /// });
    /// ```
    pub(crate) fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        let conn = self
            .inner
            .as_ref()
            .expect("Db<Open> invariant: inner must be Some")
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&conn)
    }

    /// Mutable variant, used by install/uninstall transactions.
    pub(crate) fn with_conn_mut<T>(&self, f: impl FnOnce(&mut Connection) -> T) -> T {
        let mut conn = self
            .inner
            .as_ref()
            .expect("Db<Open> invariant: inner must be Some")
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut conn)
    }

    /// Cheap clone of the inner `Arc` — used by the engine to hand
    /// the same connection to multiple sub-engines.
    pub(crate) fn share(&self) -> Arc<Mutex<Connection>> {
        self.inner
            .as_ref()
            .expect("Db<Open> invariant: inner must be Some")
            .clone()
    }
}

// ── Compile-time guarantees (documented as comments) ────────────
//
// All of these are compile errors:
//
//     let db = Db::<Closed>::new();
//     db.with_conn(|conn| { /* ... */ }); // ❌ no method on Db<Closed>
//     db.share();                          // ❌ no method on Db<Closed>
//
// And you cannot construct a `Db<Open>` without `open` or `from_arc`:
//
//     Db { inner: Some(...), _state: PhantomData } // ❌ fields are private
//
// The compiler enforces the invariant; no runtime check needed.