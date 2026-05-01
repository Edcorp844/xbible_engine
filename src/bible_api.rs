use std::sync::Arc;
use std::sync::Mutex;
use std::collections::HashMap;
use std::thread;
use crate::sword_engine::module_engine::{sword_engine::SwordEngine, sword_module::{SwordModule, ModuleBook}};
/// Download progress details for module installation
#[derive(Debug, Clone, uniffi::Record)]
pub struct DownloadProgress {
    pub progress: f64,              // 0.0 to 1.0
    pub downloaded_bytes: i64,      // Bytes downloaded so far
    pub total_bytes: i64,           // Total bytes to download
    pub current_module: String,     // Name of module being downloaded
    pub status: String,             // "downloading", "extracting", "complete", "error"
}

/// Remote module source information
#[derive(Debug, Clone, uniffi::Record)]
pub struct ModuleSource {
    pub name: String,               // e.g., "CrossWire"
    pub description: String,        // e.g., "Official SWORD Project Repository"
    pub url: String,                // Source URL
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct EngineGlobalOption {
    pub name: String,             
    pub state: String,        
}
#[derive(Debug, Clone, uniffi::Enum)]
pub enum TaskState {
    Queued,
    Running,
    Completed,
    Failed { error: String },
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct TaskStatus {
    pub task_id: String,
    pub state: TaskState,
    pub progress: f64,
    pub message: String,
}

pub struct TaskData {
    status: TaskStatus,
    result_modules: Vec<SwordModule>,
}

/// High-level Bible API abstraction layer for UniFFI export
/// Provides a clean interface for Swift and other languages to interact with Bible modules
#[derive(uniffi::Object)]
pub struct BibleEngine {
    sword_engine: Arc<SwordEngine>,
    tasks: Arc<Mutex<HashMap<String, TaskData>>>,
    next_task_id: Arc<Mutex<u64>>,
}

#[uniffi::export]
impl BibleEngine {
    /// Create a new BibleEngine instance
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sword_engine: SwordEngine::new(),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            next_task_id: Arc::new(Mutex::new(1)),
        })
    }

    /// Cancel a background task
    pub fn cancel_task(&self, task_id: String) {
        let mut tasks = self.tasks.lock().unwrap();
        if let Some(task) = tasks.get_mut(&task_id) {
            if matches!(task.status.state, TaskState::Queued | TaskState::Running) {
                task.status.state = TaskState::Failed { error: "Cancelled".to_string() };
                task.status.message = "Task cancelled".to_string();
            }
        }
    }

    /// Fetch available modules from a remote source (Asynchronous)
    /// Returns a TaskID for tracking progress
    pub fn fetch_modules_async(&self, source_name: String) -> String {
        let mut id_lock = self.next_task_id.lock().unwrap();
        let task_id = format!("task_{}", *id_lock);
        *id_lock += 1;

        let mut tasks = self.tasks.lock().unwrap();
        tasks.insert(task_id.clone(), TaskData {
            status: TaskStatus {
                task_id: task_id.clone(),
                state: TaskState::Running,
                progress: 0.0,
                message: "Fetching modules...".to_string(),
            },
            result_modules: Vec::new(),
        });

        let task_id_clone = task_id.clone();
        let tasks_clone = self.tasks.clone();
        let engine_clone = self.sword_engine.clone();

        thread::spawn(move || {
            let modules = engine_clone.fetch_remote_modules(&source_name);
            
            let mut tasks = tasks_clone.lock().unwrap();
            if let Some(task) = tasks.get_mut(&task_id_clone) {
                if let TaskState::Failed { .. } = task.status.state {
                    return;
                }
                task.status.state = TaskState::Completed;
                task.status.progress = 1.0;
                task.status.message = format!("Fetched {} modules", modules.len());
                task.result_modules = modules;
            }
        });

        task_id
    }

    /// Get the status of a background task
    pub fn get_task_status(&self, task_id: String) -> Option<TaskStatus> {
        let tasks = self.tasks.lock().unwrap();
        tasks.get(&task_id).map(|t| t.status.clone())
    }

    /// Get the modules resulting from a fetch task
    pub fn get_task_result_modules(&self, task_id: String) -> Vec<SwordModule> {
        let tasks = self.tasks.lock().unwrap();
        tasks.get(&task_id).map(|t| t.result_modules.clone()).unwrap_or_default()
    }

    /// Install a remote module from a source (Asynchronous)
    /// Returns a TaskID for tracking progress
    pub fn install_module_async(&self, source: String, module_name: String) -> String {
        let mut id_lock = self.next_task_id.lock().unwrap();
        let task_id = format!("task_{}", *id_lock);
        *id_lock += 1;

        let mut tasks = self.tasks.lock().unwrap();
        tasks.insert(task_id.clone(), TaskData {
            status: TaskStatus {
                task_id: task_id.clone(),
                state: TaskState::Running,
                progress: 0.0,
                message: format!("Installing {}...", module_name),
            },
            result_modules: Vec::new(),
        });

        let task_id_clone = task_id.clone();
        let tasks_clone = self.tasks.clone();
        let engine_clone = self.sword_engine.clone();

        thread::spawn(move || {
            let res = engine_clone.install_remote_module(&source, &module_name);
            
            let mut tasks = tasks_clone.lock().unwrap();
            if let Some(task) = tasks.get_mut(&task_id_clone) {
                if let TaskState::Failed { .. } = task.status.state {
                    return;
                }
                if res == 0 {
                    task.status.state = TaskState::Completed;
                    task.status.progress = 1.0;
                    task.status.message = format!("Successfully installed {}", module_name);
                } else {
                    task.status.state = TaskState::Failed { error: format!("Install failed with code {}", res) };
                    task.status.message = format!("Failed to install {}", module_name);
                }
            }
        });

        task_id
    }

    /// Get all available module categories
    pub fn get_available_categories(&self) -> Vec<String> {
        let mut categories: Vec<String> = self.sword_engine.get_modules().into_iter().map(|m| m.category).collect();
        categories.sort();
        categories.dedup();
        categories
    }

    /// Get all Bible modules (alias for get_available_modules for clarity)
    pub fn get_bible_modules(&self) -> Vec<SwordModule> {
        self.sword_engine.get_bible_modules()
    }

    /// Get all cult/religion study modules
    pub fn get_cult_modules(&self) -> Vec<SwordModule> {
        self.sword_engine.get_modules_by_category(vec!["Cults / Unorthodox / Questionable Material"])
    }

    /// Get all essay modules (theological essays and articles)
    pub fn get_essay_modules(&self) -> Vec<SwordModule> {
        self.sword_engine.get_modules_by_category(vec!["Essays"])
    }

    /// Get all image modules (illustrations and artwork)
    pub fn get_image_modules(&self) -> Vec<SwordModule> {
        self.sword_engine.get_modules_by_category(vec!["Images"])
    }

    /// Get all map modules
    pub fn get_map_modules(&self) -> Vec<SwordModule> {
        self.sword_engine.get_map_modules()
    }

    //set engine global options to get a module
    pub fn set_global_options(&self, options: Vec<EngineGlobalOption>) {
        unsafe {
            options.iter().for_each(|opt| {
                self.sword_engine.set_global_options(
                    &[opt.name.as_str()],
                    &opt.state.as_str(),
                )
            });
        }   
    }

    /// Get all available Bible modules
    pub fn get_available_modules(&self) -> Vec<SwordModule> {
        self.sword_engine.get_bible_modules()
    }

    /// Get the book structure for a specific module
    pub fn get_books(&self, module_name: &str) -> Vec<ModuleBook> {
        self.sword_engine.get_bible_structure(module_name)
    }

    /// Get content for a specific reference (e.g., "Genesis 1:1" or "John 3:16")
    /// using a specific module
    pub fn get_content(&self, module_name: &str, reference: &str) -> Vec<crate::sword_engine::module_engine::sword_engine_module_content_ext::Section> {
        let modules = self.sword_engine.get_bible_modules();
        if let Some(module) = modules.into_iter().find(|m| m.name == module_name) {
            self.sword_engine.get_single_entry(Some(&module), reference)
        } else {
            Vec::new()
        }
    }

    /// Get content for a whole chapter (e.g., "Genesis 1" or "John 3")
    /// using a specific module
    pub fn get_chapter_content(&self, module_name: &str, reference: &str) -> Vec<crate::sword_engine::module_engine::sword_engine_module_content_ext::Section> {
        let modules = self.sword_engine.get_bible_modules();
        if let Some(module) = modules.into_iter().find(|m| m.name == module_name) {
            self.sword_engine.get_whole_chapter(&module, reference)
        } else {
            Vec::new()
        }
    }

    /// Get all commentary modules
    pub fn get_commentary_modules(&self) -> Vec<SwordModule> {
        self.sword_engine.get_commentary_modules()
    }

    /// Get all dictionary modules
    pub fn get_dictionary_modules(&self) -> Vec<SwordModule> {
        self.sword_engine.get_dictionary_modules()
    }

    /// Get all glossary modules (simple word definitions)
    pub fn get_glossary_modules(&self) -> Vec<SwordModule> {
        self.sword_engine.get_glossary_modules()
    }

    /// Get all lexicon modules (detailed language study tools)
    pub fn get_lexicon_modules(&self) -> Vec<SwordModule> {
        self.sword_engine.get_lexicon_modules()
    }

    /// Get all daily devotional modules
    pub fn get_daily_devotional_modules(&self) -> Vec<SwordModule> {
        self.sword_engine.get_daily_devotional_modules()
    }

    /// Get all book modules (devotional books, etc.)
    pub fn get_book_modules(&self) -> Vec<SwordModule> {
        self.sword_engine.get_book_modules()
    }

    /// Install a remote module from a source
    /// Returns 0 on success, non-zero error code on failure
    pub fn install_module(&self, source: &str, module_name: &str) -> i32 {
        self.sword_engine.install_remote_module(source, module_name)
    }

    /// Get download progress (0.0 to 1.0)
    pub fn get_download_progress(&self) -> f64 {
        self.sword_engine.get_download_progress()
    }

    /// Get list of remote sources
    pub fn get_remote_sources(&self) -> Vec<String> {
        self.sword_engine.get_remote_source_list()
    }

    /// Get list of remote sources with details
    pub fn get_remote_sources_with_details(&self) -> Vec<ModuleSource> {
        let sources = self.sword_engine.get_remote_source_list();
        sources.into_iter().map(|name| {
            ModuleSource {
                name: name.clone(),
                description: self.get_source_description(&name),
                url: self.get_source_url(&name),
            }
        }).collect()
    }

    /// Get helper function for source description
    fn get_source_description(&self, source: &str) -> String {
        match source {
            "CrossWire" => "Official SWORD Project Repository".to_string(),
            "IBT" => "Institute for Bible Translation".to_string(),
            "ibiblio" => "Internet Archive Repository".to_string(),
            _ => format!("{} Repository", source),
        }
    }

    /// Get helper function for source URL
    fn get_source_url(&self, source: &str) -> String {
        match source {
            "CrossWire" => "https://crosswire.org/sword/".to_string(),
            "IBT" => "https://ibt.org.ru/sword/".to_string(),
            "ibiblio" => "https://sword.ibiblio.org/".to_string(),
            _ => format!("https://{}/sword/", source.to_lowercase()),
        }
    }

    /// Fetch available modules from a remote source
    pub fn fetch_remote_modules(&self, source_name: &str) -> Vec<SwordModule> {
        self.sword_engine.fetch_remote_modules(source_name)
    }

    /// Get detailed download progress for module installation
    pub fn get_download_progress_details(&self) -> DownloadProgress {
        let progress_value = self.sword_engine.get_download_progress();
        // Calculate bytes based on progress (this is an estimate)
        let total_bytes = 100_000_000i64; // Default estimate: 100MB
        let downloaded_bytes = (progress_value * total_bytes as f64) as i64;
        
        let status = if progress_value >= 1.0 {
            "complete".to_string()
        } else if progress_value > 0.0 {
            "downloading".to_string()
        } else {
            "waiting".to_string()
        };

        DownloadProgress {
            progress: progress_value,
            downloaded_bytes,
            total_bytes,
            current_module: String::from("xbible_engine"),
            status,
        }
    }



    /// Refresh the list of installed modules
    pub fn refresh_installed_modules(&self) -> Vec<SwordModule> {
        // This returns the updated list of all available modules after refresh
        self.sword_engine.get_modules()
    }

    /// Get installed modules by category
    pub fn get_installed_modules_by_category(&self, category: &str) -> Vec<SwordModule> {
        let all_modules = self.sword_engine.get_modules();
        all_modules.into_iter().filter(|m| m.category == category).collect()
    }

    /// Check if a module is installed
    pub fn is_module_installed(&self, module_name: &str) -> bool {
        let modules = self.sword_engine.get_modules();
        modules.iter().any(|m| m.name == module_name)
    }

    /// Get total size of all installed modules in bytes
    pub fn get_installed_modules_size(&self) -> i64 {
        let modules = self.sword_engine.get_modules();
        // Rough estimate: calculate based on module count and category
        (modules.len() as i64) * 5_000_000 // Estimate 5MB per module
    }

    /// Get information about a specific remote module
    /// Returns a TaskID for tracking progress
    pub fn get_remote_module_info(&self, source_name: &str, module_name: &str) -> Vec<SwordModule> {
        let modules = self.sword_engine.fetch_remote_modules(source_name);
        modules.into_iter().filter(|m| m.name == module_name).collect()
    }

    /// Search for modules matching a query across all sources
    pub fn search_modules(&self, source_name: &str, query: &str) -> Vec<SwordModule> {
        let modules = self.sword_engine.fetch_remote_modules(source_name);
        let query_lower = query.to_lowercase();
        
        modules.into_iter().filter(|m| {
            m.name.to_lowercase().contains(&query_lower) ||
            m.description.to_lowercase().contains(&query_lower)
        }).collect()
    }

    /// Get modules by language
    pub fn get_modules_by_language(&self, language_code: &str, source_name: &str) -> Vec<SwordModule> {
        let modules = self.sword_engine.fetch_remote_modules(source_name);
        modules.into_iter().filter(|m| m.language.contains(language_code)).collect()
    }


}

// Re-export the data structures for UniFFI
pub use crate::sword_engine::module_engine::sword_engine_module_content_ext::{Section, Verse, Word, LexicalInfo, TextDirection};