uniffi::setup_scaffolding!();

#[allow(non_camel_case_types, non_upper_case_globals, non_snake_case, dead_code)]
pub mod ffi {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

pub mod sword_engine;
pub use sword_engine::SwordEngine;
