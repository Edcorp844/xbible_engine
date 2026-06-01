use std::{thread, time::Duration};

use crate::engines::{
    module_engine::{
        module_engine_extensions::{
            module_engine_dictionary_ext::{DictionaryQuery, DictionaryResponse},
            module_engine_lexicon_ext::{LexiconQuery, LexiconResponse},
            module_engine_module_content_ext::Section,
        },
        sword_module::{module::SwordModule, module_book::ModuleBook},
    },
    xbible_engine::{
        engine::XBibleEngine,
        xbible_engine_extensions::xbible_engine_task_ext::{TaskData, TaskState, TaskStatus},
    },
};

/// Remote module source information
#[derive(Debug, Clone, uniffi::Record)]
pub struct ModuleSource {
    pub name: String,        // e.g., "CrossWire"
    pub description: String, // e.g., "Official SWORD Project Repository"
    pub url: String,         // Source URL
}


#[derive(Debug, Clone, uniffi::Record)]
pub struct EngineGlobalOption {
    pub name: String,
    pub state: String,
}

#[uniffi::export]
impl XBibleEngine {
    /// Fetch available modules from a remote source (Asynchronous)
    /// Returns a TaskID for tracking progress
    pub fn fetch_modules_async(&self, source_name: String) -> String {
        self.fetch_multiple_sources_async(vec![source_name])
    }

    /// Fetch available modules from multiple remote sources in parallel (Asynchronous)
    /// Returns a TaskID for tracking progress
    pub fn fetch_multiple_sources_async(&self, sources: Vec<String>) -> String {
        let mut id_lock = self.next_task_id.lock().unwrap();
        let task_id = format!("task_{}", *id_lock);
        *id_lock += 1;

        let mut tasks = self.tasks.lock().unwrap();
        tasks.insert(
            task_id.clone(),
            TaskData {
                status: TaskStatus {
                    task_id: task_id.clone(),
                    state: TaskState::Running,
                    progress: 0.0,
                    message: format!("Preparing to fetch from {} sources...", sources.len()),
                },
                result_modules: Vec::new(),
            },
        );

        let task_id_clone = task_id.clone();
        let tasks_clone = self.tasks.clone();
        let module_engine = self.module_engine.clone();

        thread::spawn(move || {
            let total_sources = sources.len();
            let mut all_modules = Vec::new();
            let mut handles = Vec::new();

            // Spawn a thread for each source for true parallel fetching
            for source in sources {
                let se = module_engine.clone();
                let s = source.clone();
                handles.push(thread::spawn(move || se.fetch_remote_modules(&s)));
            }

            // Wait for all fetches and update progress incrementally
            let mut completed = 0;
            for handle in handles {
                if let Ok(mods) = handle.join() {
                    all_modules.extend(mods);
                }
                completed += 1;

                let mut tasks = tasks_clone.lock().unwrap();
                if let Some(task) = tasks.get_mut(&task_id_clone) {
                    if let TaskState::Running = task.status.state {
                        task.status.progress = completed as f64 / total_sources as f64;
                        task.status.message =
                            format!("Fetched {}/{} sources...", completed, total_sources);
                    }
                }
            }

            let mut tasks = tasks_clone.lock().unwrap();
            if let Some(task) = tasks.get_mut(&task_id_clone) {
                if let TaskState::Failed { .. } = task.status.state {
                    return;
                }
                task.status.state = TaskState::Completed;
                task.status.progress = 1.0;
                task.status.message = format!(
                    "Fetched {} modules from {} sources",
                    all_modules.len(),
                    total_sources
                );
                task.result_modules = all_modules;
            }
        });

        task_id
    }

    /// Install a remote module from a source (Asynchronous)
    /// Returns a TaskID for tracking progress
    pub fn install_module_async(&self, source: String, module_name: String) -> String {
        let mut id_lock = self.next_task_id.lock().unwrap();
        let task_id = format!("task_{}", *id_lock);
        *id_lock += 1;

        let mut tasks = self.tasks.lock().unwrap();
        tasks.insert(
            task_id.clone(),
            TaskData {
                status: TaskStatus {
                    task_id: task_id.clone(),
                    state: TaskState::Running,
                    progress: 0.0,
                    message: format!("Installing {}...", module_name),
                },
                result_modules: Vec::new(),
            },
        );

        let task_id_clone = task_id.clone();
        let tasks_clone = self.tasks.clone();
        let engine_clone = self.module_engine.clone();

        thread::spawn(move || {
            // Use a separate thread for the blocking install call so we can poll progress
            let engine_for_install = engine_clone.clone();
            let source_for_install = source.clone();
            let module_for_install = module_name.clone();

            let install_handle = thread::spawn(move || {
                engine_for_install.install_remote_module(&source_for_install, &module_for_install)
            });

            // Poll progress until the install thread finishes
            while !install_handle.is_finished() {
                let current_progress = engine_clone.get_download_progress();

                let mut tasks = tasks_clone.lock().unwrap();
                if let Some(task) = tasks.get_mut(&task_id_clone) {
                    // Only update if not already failed/cancelled
                    if let TaskState::Running = task.status.state {
                        task.status.progress = current_progress;
                        task.status.message = format!(
                            "Downloading {}: {:.1}%",
                            module_name,
                            current_progress * 100.0
                        );
                    } else {
                        // Task was cancelled or failed by another thread
                        break;
                    }
                }
                drop(tasks);
                thread::sleep(Duration::from_millis(250));
            }

            let res = install_handle.join().unwrap_or(-1);

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
                    task.status.state = TaskState::Failed {
                        error: format!("Install failed with code {}", res),
                    };
                    task.status.message = format!("Failed to install {}", module_name);
                }
            }
        });

        task_id
    }

    /// Uninstall a module (Asynchronous)
    /// Returns a TaskID for tracking progress
    pub fn uninstall_module_async(&self, module_name: String) -> String {
        let mut id_lock = self.next_task_id.lock().unwrap();
        let task_id = format!("task_{}", *id_lock);
        *id_lock += 1;

        let mut tasks = self.tasks.lock().unwrap();
        tasks.insert(
            task_id.clone(),
            TaskData {
                status: TaskStatus {
                    task_id: task_id.clone(),
                    state: TaskState::Running,
                    progress: 0.0,
                    message: format!("Uninstalling {}...", module_name),
                },
                result_modules: Vec::new(),
            },
        );

        let task_id_clone = task_id.clone();
        let tasks_clone = self.tasks.clone();
        let engine_clone = self.module_engine.clone();

        thread::spawn(move || {
            let res = engine_clone.uninstall_module(&module_name);

            let mut tasks = tasks_clone.lock().unwrap();
            if let Some(task) = tasks.get_mut(&task_id_clone) {
                if let TaskState::Failed { .. } = task.status.state {
                    return;
                }
                if res == 0 {
                    task.status.state = TaskState::Completed;
                    task.status.progress = 1.0;
                    task.status.message = format!("Successfully uninstalled {}", module_name);
                } else {
                    task.status.state = TaskState::Failed {
                        error: format!("Uninstall failed with code {}", res),
                    };
                    task.status.message = format!("Failed to uninstall {}", module_name);
                }
            }
        });

        task_id
    }

    /// Get all available module categories
    pub fn get_available_categories(&self) -> Vec<String> {
        let mut categories: Vec<String> = self
            .module_engine
            .get_modules()
            .into_iter()
            .map(|m| m.category)
            .collect();
        categories.sort();
        categories.dedup();
        categories
    }

    /// Get all Bible modules (alias for get_available_modules for clarity)
    pub fn get_bible_modules(&self) -> Vec<SwordModule> {
        self.module_engine.get_bible_modules()
    }

    /// Get all cult/religion study modules
    pub fn get_cult_modules(&self) -> Vec<SwordModule> {
        self.module_engine
            .get_modules_by_category(vec!["Cults / Unorthodox / Questionable Material"])
    }

    /// Get all essay modules (theological essays and articles)
    pub fn get_essay_modules(&self) -> Vec<SwordModule> {
        self.module_engine.get_modules_by_category(vec!["Essays"])
    }

    /// Get all image modules (illustrations and artwork)
    pub fn get_image_modules(&self) -> Vec<SwordModule> {
        self.module_engine.get_modules_by_category(vec!["Images"])
    }

    /// Get all map modules
    pub fn get_map_modules(&self) -> Vec<SwordModule> {
        self.module_engine.get_map_modules()
    }

    //set engine global options to get a module
    pub fn set_global_options(&self, options: Vec<EngineGlobalOption>) {
        unsafe {
            options.iter().for_each(|opt| {
                self.module_engine
                    .set_global_options(&[opt.name.as_str()], &opt.state.as_str())
            });
        }
    }

    /// Get all available Bible modules
    pub fn get_available_modules(&self) -> Vec<SwordModule> {
        self.module_engine.get_bible_modules()
    }

    /// Get the book structure for a specific module
    pub fn get_books(&self, module_name: &str) -> Vec<ModuleBook> {
        self.module_engine.get_bible_structure(module_name)
    }

    // Look up a word in the dictionary modules for a specific language and return definitions
    pub fn lookup_dictionary(&self, query: DictionaryQuery) -> DictionaryResponse {
        self.module_engine.lookup_dictionary(query)
    }

    // Look up a Strong's number in the lexicon modules for a specific language and return detailed information
    pub fn lookup_strongs_number(&self, query: LexiconQuery) -> LexiconResponse {
        self.module_engine.lookup_strongs_number(query)
    }

    /// Get content for a specific reference (e.g., "Genesis 1:1" or "John 3:16")
    /// using a specific module
    pub fn get_content(&self, module_name: &str, reference: &str) -> Vec<Section> {
        let modules = self.module_engine.get_modules();
        if let Some(module) = modules.into_iter().find(|m| m.name == module_name) {
            self.module_engine
                .get_single_entry(Some(&module), reference)
        } else {
            Vec::new()
        }
    }

    /// Get content for a whole chapter (e.g., "Genesis 1" or "John 3")
    /// using a specific module
    pub fn get_chapter_content(&self, module_name: &str, reference: &str) -> Vec<Section> {
        let modules = self.module_engine.get_modules();
        if let Some(module) = modules.into_iter().find(|m| m.name == module_name) {
            self.module_engine.get_whole_chapter(&module, reference)
        } else {
            Vec::new()
        }
    }

    /// Get all commentary modules
    pub fn get_commentary_modules(&self) -> Vec<SwordModule> {
        self.module_engine.get_commentary_modules()
    }

    /// Get all dictionary modules
    pub fn get_dictionary_modules(&self) -> Vec<SwordModule> {
        self.module_engine.get_dictionary_modules()
    }

    /// Get all glossary modules (simple word definitions)
    pub fn get_glossary_modules(&self) -> Vec<SwordModule> {
        self.module_engine.get_glossary_modules()
    }

    /// Get all lexicon modules (detailed language study tools)
    pub fn get_lexicon_modules(&self) -> Vec<SwordModule> {
        self.module_engine.get_lexicon_modules()
    }

    /// Get all daily devotional modules
    pub fn get_daily_devotional_modules(&self) -> Vec<SwordModule> {
        self.module_engine.get_daily_devotional_modules()
    }

    /// Get all book modules (devotional books, etc.)
    pub fn get_book_modules(&self) -> Vec<SwordModule> {
        self.module_engine.get_book_modules()
    }

    /// Install a remote module from a source
    /// Returns 0 on success, non-zero error code on failure
    pub fn install_module(&self, source: &str, module_name: &str) -> i32 {
        self.module_engine
            .install_remote_module(source, module_name)
    }

    /// Uninstall a module
    /// Returns 0 on success, non-zero error code on failure
    pub fn uninstall_module(&self, module_name: &str) -> i32 {
        self.module_engine.uninstall_module(module_name)
    }

    /// Get download progress (0.0 to 1.0)
    pub fn get_download_progress(&self) -> f64 {
        self.module_engine.get_download_progress()
    }

    /// Get list of remote sources
    pub fn get_remote_sources(&self) -> Vec<String> {
        self.module_engine.get_remote_source_list()
    }

    /// Get list of remote sources with details
    pub fn get_remote_sources_with_details(&self) -> Vec<ModuleSource> {
        let sources = self.module_engine.get_remote_source_list();
        sources
            .into_iter()
            .map(|name| ModuleSource {
                name: name.clone(),
                description: self.get_source_description(&name),
                url: self.get_source_url(&name),
            })
            .collect()
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
        self.module_engine.fetch_remote_modules(source_name)
    }

    /// Refresh the list of installed modules
    pub fn refresh_installed_modules(&self) -> Vec<SwordModule> {
        // This returns the updated list of all available modules after refresh
        self.module_engine.get_modules()
    }

    /// Get installed modules by category
    pub fn get_installed_modules_by_category(&self, category: &str) -> Vec<SwordModule> {
        let all_modules = self.module_engine.get_modules();
        all_modules
            .into_iter()
            .filter(|m| m.category == category)
            .collect()
    }

    /// Check if a module is installed
    pub fn is_module_installed(&self, module_name: &str) -> bool {
        let modules = self.module_engine.get_modules();
        modules.iter().any(|m| m.name == module_name)
    }

    /// Get total size of all installed modules in bytes
    pub fn get_installed_modules_size(&self) -> i64 {
        let modules = self.module_engine.get_modules();
        // Rough estimate: calculate based on module count and category
        (modules.len() as i64) * 5_000_000 // Estimate 5MB per module
    }

    /// Get information about a specific remote module
    /// Returns a TaskID for tracking progress
    pub fn get_remote_module_info(&self, source_name: &str, module_name: &str) -> Vec<SwordModule> {
        let modules = self.module_engine.fetch_remote_modules(source_name);
        modules
            .into_iter()
            .filter(|m| m.name == module_name)
            .collect()
    }

    /// Search for modules matching a query across all sources
    pub fn search_modules(&self, source_name: &str, query: &str) -> Vec<SwordModule> {
        let modules = self.module_engine.fetch_remote_modules(source_name);
        let query_lower = query.to_lowercase();

        modules
            .into_iter()
            .filter(|m| {
                m.name.to_lowercase().contains(&query_lower)
                    || m.description.to_lowercase().contains(&query_lower)
            })
            .collect()
    }

    /// Get modules by language
    pub fn get_modules_by_language(
        &self,
        language_code: &str,
        source_name: &str,
    ) -> Vec<SwordModule> {
        let modules = self.module_engine.fetch_remote_modules(source_name);
        modules
            .into_iter()
            .filter(|m| m.language.contains(language_code))
            .collect()
    }

    /// Get a single module entry by name
    pub fn get_single_entry(&self, sword_module: &SwordModule, reference: &str) -> Vec<Section>{
        self.module_engine.get_single_entry(Some(sword_module), reference)
    }
}
