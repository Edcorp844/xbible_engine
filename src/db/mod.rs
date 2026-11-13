use rusqlite::Connection;

pub mod connection;
pub mod connection_state;
pub mod error;
mod migrations;
pub mod writer;

use error::DbError;

pub fn open(path: &str) -> Result<Connection, DbError> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5_000_i64)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrations::apply(&mut conn)?;
    Ok(conn)
}