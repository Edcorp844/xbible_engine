//! Sword Engine Module
//!
//! This module provides a high-level Rust interface to the Sword library,
//! which is used for accessing Bible texts and related functionality.

pub mod engine;
pub mod module;
#[cfg(test)]
mod tests;

pub use engine::SwordEngine;
pub use module::{ModuleBook, ModuleChapter, SwordModule};