pub(crate) fn initialize_logging() {
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

    log::info!(
        "Initialized logging for XBibleEngine on {}",
        std::env::consts::OS
    );
}

/// FFI-exported entrypoint. Kept as a free function so the generated Kotlin
/// bindings get `uniffi_xbible_engine_fn_func_init_logging` (and its checksum).
/// The engine constructor also calls `initialize_logging()` so logging is set
/// up automatically when `XBibleEngine()` is constructed.
#[uniffi::export]
pub fn init_logging() {
    initialize_logging();
}