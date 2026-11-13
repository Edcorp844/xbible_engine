use std::sync::Arc;
use std::sync::Mutex;

use crate::engines::audio_engine::engine::AudioEngine;
use crate::engines::freebible_engine::FreeBibleEngine;

/// Top-level API object exposed to foreign-language consumers.
/// Feature-specific engine handles are accessed through this facade.
#[allow(dead_code)]
#[derive(Debug, uniffi::Object)]
pub struct XBibleEngine {
    next_task_id: Arc<Mutex<u64>>,
    audio_engine: Arc<AudioEngine>,
    freebible_engine: Arc<FreeBibleEngine>,
}

#[uniffi::export]
impl XBibleEngine {
    /// Create a new XBibleEngine instance and initialize platform logging.
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        super::initialization::init_logging();
        Arc::new(Self {
            next_task_id: Arc::new(Mutex::new(1)),
            audio_engine: AudioEngine::new(),
            freebible_engine: FreeBibleEngine::new(),
        })
    }

    /// Audio feature handle.
    pub fn audio_engine(&self) -> Arc<AudioEngine> {
        self.audio_engine.clone()
    }

    /// The FreeBible sub-engine (store + reader).
    pub fn freebible(&self) -> Arc<FreeBibleEngine> {
        self.freebible_engine.clone()
    }
}