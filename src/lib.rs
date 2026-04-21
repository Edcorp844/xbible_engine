
uniffi::setup_scaffolding!();


#[allow(non_camel_case_types, non_upper_case_globals, non_snake_case, dead_code)]
pub mod ffi {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

#[uniffi::export]
pub fn make_sentence(words: Vec<String>) -> String{
    words.join(" ")
}

pub mod sword_engine;
pub mod bible_api;
