use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use crate::engines::module_engine::module_engine::ModuleEngine;
use crate::engines::xbible_engine::xbible_engine_extensions::xbible_engine_task_ext::TaskData;




/// High-level Bible API abstraction layer for UniFFI export
/// Provides a clean interface for Swift and other languages to interact with Bible modules
#[derive(uniffi::Object)]
pub struct XBibleEngine {
    pub(crate) module_engine: Arc<ModuleEngine>,
    pub(crate) tasks: Arc<Mutex<HashMap<String, TaskData>>>,
    pub(crate) next_task_id: Arc<Mutex<u64>>,
}

#[uniffi::export]
impl XBibleEngine {
    /// Create a new BibleEngine instance
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            module_engine: ModuleEngine::new(),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            next_task_id: Arc::new(Mutex::new(1)),
        })
    }
}
