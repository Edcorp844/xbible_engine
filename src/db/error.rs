// crate/src/db/error.rs

//! Errors raised by the shared database layer.
//!
//! Kept independent of any engine so that every feature (FreeBible,
//! SWORD, timeline, users, …) can consume the DB without pulling in
//! engine-specific error types.

use std::fmt;

/// Anything that can go wrong opening, migrating, or operating on the
/// shared SQLite database.
#[derive(Debug)]
pub enum DbError {
    /// `rusqlite` returned an error.
    Sqlite(rusqlite::Error),

    /// The migration runner failed.
    Migration(rusqlite_migration::Error),

    /// File system error — the DB path could not be opened.
    Io(std::io::Error),

    /// Something else, wrapped as a message.
    Other(String),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "sqlite: {e}"),
            Self::Migration(e) => write!(f, "migration: {e}"),
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for DbError {}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<rusqlite_migration::Error> for DbError {
    fn from(e: rusqlite_migration::Error) -> Self {
        Self::Migration(e)
    }
}

impl From<std::io::Error> for DbError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<String> for DbError {
    fn from(s: String) -> Self {
        Self::Other(s)
    }
}

impl From<&str> for DbError {
    fn from(s: &str) -> Self {
        Self::Other(s.to_string())
    }
}