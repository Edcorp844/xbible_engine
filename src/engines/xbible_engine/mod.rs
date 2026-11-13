//! Public UniFFI facade. Feature engines are reached through this object.

pub(crate) mod initialization;
pub mod engine;

pub use engine::XBibleEngine;