use std::sync::Arc;
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

/// High-level Bible API abstraction layer for UniFFI export
/// Provides a clean interface for Swift and other languages to interact with Bible modules
#[derive(uniffi::Object)]
pub struct BibleEngine {
    sword_engine: Arc<SwordEngine>,
}

#[uniffi::export]
impl BibleEngine {
    /// Create a new BibleEngine instance
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            sword_engine: SwordEngine::new(),
        })
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

    /// Install a remote module from a source with detailed progress tracking
    /// Returns 0 on success, non-zero error code on failure
    pub fn install_module_with_progress(&self, source: &str, module_name: &str) -> i32 {
        println!("[BibleEngine] Starting installation of {} from {}", module_name, source);
        self.sword_engine.install_remote_module(source, module_name)
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
    pub fn get_remote_module_info(&self, source_name: &str, module_name: &str) -> Option<SwordModule> {
        let modules = self.sword_engine.fetch_remote_modules(source_name);
        modules.into_iter().find(|m| m.name == module_name)
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