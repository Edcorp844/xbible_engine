#![allow(unknown_lints)]
#![allow(unsafe_attr_outside_unsafe)]

uniffi::setup_scaffolding!();

#[allow(non_camel_case_types, non_upper_case_globals, non_snake_case, dead_code)]
pub mod ffi {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}

#[macro_use]
extern crate log;

#[uniffi::export]
#[unsafe(no_mangle)]
pub extern "C" fn init_logging() {
    #[cfg(target_os = "android")]
    {
        android_logger::init_once(
            android_logger::Config::default()
                .with_max_level(log::LevelFilter::Debug)
                .with_tag("XBible Engine"),
        );
    }

    #[cfg(not(target_os = "android"))]
    {
        // Try initializing terminal logger first (captures stdout for cargo run/test on macOS/Linux/Windows)
        let env_init = env_logger::builder()
            .filter_level(log::LevelFilter::Debug)
            .is_test(true)
            .try_init();

        #[cfg(any(target_os = "ios", target_os = "macos"))]
        if env_init.is_err() {
            let _ = oslog::OsLogger::new("com.xbible.engine")
                .level_filter(log::LevelFilter::Debug)
                .init();
        }
    }

    log::info!("Initialized logging for XBibleEngine on {}", std::env::consts::OS);
}

pub mod engines;
pub mod runtime;
pub mod data;