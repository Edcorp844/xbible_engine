#![allow(unknown_lints)]
#![allow(unsafe_attr_outside_unsafe)]

uniffi::setup_scaffolding!();

#[macro_use]
extern crate log;

/// Initialize logging when using the Rust API directly.
///
/// Foreign-language consumers get logging initialization from `XBibleEngine::new`.
pub fn init_logging() {
    engines::xbible_engine::initialization::initialize_logging();
}

/// Required when libbz2 is built with BZ_NO_STDIO (typical on iOS).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn bz_internal_error(errcode: std::os::raw::c_int) {
    // Avoid depending on stdio inside the bz2 library path.
    error!("bzip2 internal error: {errcode}");
    std::process::abort();
}

pub mod data;
pub mod db;
pub mod engines;
pub mod runtime;

pub use engines::XBibleEngine;
