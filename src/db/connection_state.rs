// freebible_engine/state.rs

//! Connection states, expressed as types.
//!
//! `Closed` and `Open` are zero-sized markers. They're used as the
//! type parameter of `Db<S>`, `Reader<S>`, `StoreEngine<S>`, and
//! `Engine<S>`. Methods that need a live database exist only on
//! `S = Open`, so the compiler rejects any attempt to query or write
//! before the database is opened.
//!
//! The trait is sealed: only `Closed` and `Open` exist, and only
//! this module can add more. That means downstream code can match
//! on states exhaustively and never has to worry about a third
//! variant appearing.

/// Marker trait implemented by every connection state.
///
/// Sealed via [`sealed::Sealed`] — external crates cannot add states.

pub trait ConnectionState: sealed::Sealed {}

/// No live database. Only `open` is available.
///
/// Consuming a `Db<Closed>` produces a `Db<Open>`. After that, the
/// `Closed` value no longer exists — you cannot accidentally reuse
/// a stale handle.
#[derive(Debug)]
pub struct Closed;

/// Live database. Every query and write method is available.
///
/// There is no `Db<Closed>::from(Db<Open>)` — you can't downgrade
/// silently. If you ever need to close a database, that's an
/// explicit operation on the engine (drop the `Db<Open>` and start
/// a fresh `Db<Closed>`).
#[derive(Debug)]
pub struct Open;

impl ConnectionState for Closed {}
impl ConnectionState for Open {}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::Closed {}
    impl Sealed for super::Open {}
}