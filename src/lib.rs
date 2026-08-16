
uniffi::setup_scaffolding!();


#[allow(non_camel_case_types, non_upper_case_globals, non_snake_case, dead_code)]
pub mod ffi {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

#[macro_use]
extern crate log;

pub mod engines;
pub mod runtime;
pub mod data;
