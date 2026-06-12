use std::sync::OnceLock;
use tokio::runtime::Runtime;

/// Returns a global reference to a shared, background multi-threaded Tokio runtime.
/// This ensures a reactor is always alive and available to the underlying network tasks.
pub fn get_shared_runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("Failed to initialize structural engine Tokio execution pool")
    })
}