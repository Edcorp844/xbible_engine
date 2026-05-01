use directories::ProjectDirs;
use std::ffi::{CStr, CString};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::ffi::*;
use crate::sword_engine::module_engine::sword_module::{ModuleBook, ModuleChapter, SwordModule};



static PROGRESS_TOTAL: AtomicU64 = AtomicU64::new(0);
static PROGRESS_COMPLETED: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct SwordInner {
    pub mgr: isize,
    pub install_mgr: isize,
}

#[derive(Debug)]
pub struct SwordEngine {
    pub inner: Mutex<SwordInner>,
    pub sword_path: PathBuf,
}

impl SwordEngine {
    pub fn new() -> Arc<Self> {
        let path = Self::get_sword_path();

        // Pre-create folders BEFORE initializing the C handles
        Self::prepare_app_directory(&path);

        let path_str = path.to_string_lossy().replace("\\", "/");
        let c_path = CString::new(path_str.clone()).unwrap();

        unsafe {
            println!("[SwordEngine] Initializing InstallMgr at: {}", path_str);
            let install_mgr =
                org_crosswire_sword_InstallMgr_new(c_path.as_ptr(), Some(Self::status_reporter));

            // Force disclaimer and a baseline sync
            org_crosswire_sword_InstallMgr_setUserDisclaimerConfirmed(install_mgr);
            org_crosswire_sword_InstallMgr_syncConfig(install_mgr);

            println!("[SwordEngine] Initializing SWMgr...");
            let mgr = org_crosswire_sword_SWMgr_newWithPath(c_path.as_ptr());

            let utf8_key = CString::new("UTF8").unwrap();
            let on_val = CString::new("true").unwrap();
            org_crosswire_sword_SWMgr_setGlobalOption(mgr, utf8_key.as_ptr(), on_val.as_ptr());

            Arc::new(Self {
                inner: Mutex::new(SwordInner { mgr, install_mgr }),
                sword_path: path,
            })
        }
    }

    unsafe extern "C" fn status_reporter(
        msg: *const ::std::os::raw::c_char,
        total: ::std::os::raw::c_ulong,
        completed: ::std::os::raw::c_ulong,
    ) {
        unsafe {
            PROGRESS_TOTAL.store(total as u64, Ordering::SeqCst);
            PROGRESS_COMPLETED.store(completed as u64, Ordering::SeqCst);

            if !msg.is_null() {
                let message = CStr::from_ptr(msg).to_string_lossy();
                println!(
                    "[SwordEngine] Progress: {}/{} - {}",
                    completed, total, message
                );
            }
        }
    }

    unsafe fn rebuild_mgr(&self, inner: &mut SwordInner) {
        println!("[SwordEngine] Rebuilding SWMgr...");
        unsafe { org_crosswire_sword_SWMgr_delete(inner.mgr) };

        let path_str = self.sword_path.to_string_lossy().replace("\\", "/");
        let c_path = CString::new(path_str).unwrap();

        inner.mgr = unsafe { org_crosswire_sword_SWMgr_newWithPath(c_path.as_ptr()) };

        let utf8_key = CString::new("UTF8").unwrap();
        let on_val = CString::new("true").unwrap();
        unsafe {
            org_crosswire_sword_SWMgr_setGlobalOption(inner.mgr, utf8_key.as_ptr(), on_val.as_ptr())
        };

        // Also sync the InstallMgr config to refresh local module detection
        unsafe {
            org_crosswire_sword_InstallMgr_syncConfig(inner.install_mgr);
        };

        println!("[SwordEngine] SWMgr rebuilt successfully");
    }

    // ------------------- REMOTE SOURCES -------------------

    pub fn get_remote_source_list(&self) -> Vec<String> {
        let inner = self.inner.lock().unwrap();
        let mut sources = Vec::new();
        unsafe {
            org_crosswire_sword_InstallMgr_setUserDisclaimerConfirmed(inner.install_mgr);
            org_crosswire_sword_InstallMgr_syncConfig(inner.install_mgr);

            let ptr = org_crosswire_sword_InstallMgr_getRemoteSources(inner.install_mgr);
            if !ptr.is_null() {
                let mut i = 0;
                while !(*ptr.offset(i)).is_null() {
                    sources.push(self.ptr_to_str(*ptr.offset(i)));
                    i += 1;
                }
            }
        }

        // If no sources were found (network issues, permissions, etc.), provide defaults
        if sources.is_empty() {
            println!("[SwordEngine] No remote sources found, using default sources");
            sources = vec![
                "CrossWire".to_string(),
                "IBT".to_string(),
                "ibiblio".to_string(),
            ];
        }

        println!("[SwordEngine] Remote sources: {:?}", sources);
        sources
    }

    pub fn fetch_remote_modules(&self, source_name: &str) -> Vec<SwordModule> {
        let mut modules = Vec::new();
        let c_source = CString::new(source_name).unwrap();
        
        let path_str = self.sword_path.to_string_lossy().replace("\\", "/");
        let c_path = CString::new(path_str).unwrap();

        unsafe {
            // Create local, temporary handles for this fetch task.
            // This allows us to perform multiple fetches in parallel without locking the global engine.
            let local_install_mgr = org_crosswire_sword_InstallMgr_new(c_path.as_ptr(), None);
            let local_mgr = org_crosswire_sword_SWMgr_newWithPath(c_path.as_ptr());

            // 1. Refresh (Downloads to temp)
            org_crosswire_sword_InstallMgr_setUserDisclaimerConfirmed(local_install_mgr);
            org_crosswire_sword_InstallMgr_refreshRemoteSource(
                local_install_mgr,
                c_source.as_ptr(),
            );

            // 2. Sync (Moves from temp to InstallMgr/RemoteSources)
            org_crosswire_sword_InstallMgr_syncConfig(local_install_mgr);
            org_crosswire_sword_InstallMgr_syncConfig(local_install_mgr);

            // 3. Query the remote list
            let info_ptr = org_crosswire_sword_InstallMgr_getRemoteModInfoList(
                local_install_mgr,
                local_mgr,
                c_source.as_ptr(),
            );

            if !info_ptr.is_null() {
                let mut i = 0;
                loop {
                    let entry = info_ptr.offset(i);
                    if entry.is_null() || (*entry).name.is_null() {
                        break;
                    }
                    let mut features_vec = Vec::new();
                    let feature_ptr_ptr = (*entry).features;

                    if !feature_ptr_ptr.is_null() {
                        let mut j = 0;
                        while !(*feature_ptr_ptr.offset(j)).is_null() {
                            let feature_c_str = CStr::from_ptr(*feature_ptr_ptr.offset(j));
                            features_vec.push(feature_c_str.to_string_lossy().into_owned());
                            j += 1;
                        }
                    }

                    modules.push(SwordModule {
                        name: self.ptr_to_str((*entry).name),
                        description: self.ptr_to_str((*entry).description),
                        category: self.ptr_to_str((*entry).category),
                        language: self.from_code(self.ptr_to_str((*entry).language).as_str()),
                        source: source_name.to_string(),
                        version: self.ptr_to_str((*entry).version),
                        delta: self.ptr_to_str((*entry).delta),
                        cipher_key: self.ptr_to_str((*entry).cipherKey),
                        features: features_vec,
                    });
                    i += 1;
                }
            }

            // Cleanup local handles
            org_crosswire_sword_SWMgr_delete(local_mgr);
            org_crosswire_sword_InstallMgr_delete(local_install_mgr);
        }
        modules
    }
    // ------------------- LOCAL MODULES -------------------

    pub fn get_modules(&self) -> Vec<SwordModule> {
        let mut modules = Vec::new();
        let inner = self.inner.lock().unwrap();

        unsafe {
            let mut ptr = org_crosswire_sword_SWMgr_getModInfoList(inner.mgr);

            while !ptr.is_null() && !(*ptr).name.is_null() {
                let info = *ptr;

                // --- CONVERT FEATURES ARRAY TO VEC<STRING> ---
                let mut features_vec = Vec::new();
                let feature_ptr_ptr = info.features; // *mut *const c_char

                if !feature_ptr_ptr.is_null() {
                    let mut i = 0;
                    // Loop until the pointer at the current offset is null
                    while !(*feature_ptr_ptr.offset(i)).is_null() {
                        let feature_c_str = CStr::from_ptr(*feature_ptr_ptr.offset(i));
                        features_vec.push(feature_c_str.to_string_lossy().into_owned());
                        i += 1;
                    }
                }

                modules.push(SwordModule {
                    name: self.ptr_to_str(info.name),
                    description: self.ptr_to_str(info.description),
                    category: self.ptr_to_str(info.category),
                    language: self.from_code(self.ptr_to_str(info.language).as_str()),
                    source: "Local".to_string(),
                    version: self.ptr_to_str(info.version),
                    delta: self.ptr_to_str(info.delta),
                    cipher_key: self.ptr_to_str(info.cipherKey),
                    features: features_vec, // Assign the Vec<String> here
                });

                ptr = ptr.offset(1);
            }
        }

        //println!("[SwordEngine] Local modules found: {:?}", modules);
        modules
    }

    pub fn get_modules_by_category(&self, categories: Vec<&str>) -> Vec<SwordModule> {
        let modules = self
            .get_modules()
            .into_iter()
            .filter(|m| categories.contains(&m.category.as_str()))
            .collect();

        //println!("MODULES: {:?}", modules);

        modules
    }

    pub fn get_bible_modules(&self) -> Vec<SwordModule> {
        self.get_modules_by_category(vec!["Biblical Texts", "Bibles"])
    }

    pub fn get_commentary_modules(&self) -> Vec<SwordModule> {
        self.get_modules_by_category(vec!["Commentaries"])
    }

    pub fn get_dictionary_modules(&self) -> Vec<SwordModule> {
        self.get_modules()
            .into_iter()
            .filter(|m| {
                let cat = m.category.to_lowercase();
                let name = m.name.to_lowercase();

                // 1. Must be a dictionary-type category
                let is_dict_cat = cat.contains("dict")
                    || cat.contains("lex")
                    || cat.contains("gloss")
                    || cat.contains("daily");

                // 2. Must NOT be a Bible
                let is_bible = cat.contains("bible") || cat.contains("text");

                // 3. Or it's a known Strong's dictionary name
                let is_strongs_name = name.contains("strong") && !is_bible;

                // Logic: It must be a dictionary category OR a Strong's named module,
                // but it absolutely cannot be a Bible text module.
                (is_dict_cat || is_strongs_name) && !is_bible
            })
            .collect()
    }

    /// Get glossary modules (simple word definitions)
    pub fn get_glossary_modules(&self) -> Vec<SwordModule> {
        self.get_modules()
            .into_iter()
            .filter(|m| {
                let cat = m.category.to_lowercase();
                let desc = m.description.to_lowercase();

                // Glossaries typically have "gloss" in category or are simple dictionaries
                cat.contains("gloss") ||
                (cat.contains("dict") && !desc.contains("lexicon"))
            })
            .collect()
    }

    /// Get lexicon modules (detailed language study tools)
    pub fn get_lexicon_modules(&self) -> Vec<SwordModule> {
        self.get_modules()
            .into_iter()
            .filter(|m| {
                let cat = m.category.to_lowercase();
                let name = m.name.to_lowercase();
                let desc = m.description.to_lowercase();

                // Lexicons have "lex" in category or are Strong's dictionaries
                cat.contains("lex") ||
                name.contains("strong") ||
                desc.contains("lexicon") ||
                desc.contains("strong")
            })
            .collect()
    }

    /// Get daily devotional modules
    pub fn get_daily_devotional_modules(&self) -> Vec<SwordModule> {
        self.get_modules()
            .into_iter()
            .filter(|m| {
                let cat = m.category.to_lowercase();
                let desc = m.description.to_lowercase();

                cat.contains("daily") ||
                desc.contains("devotional") ||
                desc.contains("daily")
            })
            .collect()
    }

    pub fn get_book_modules(&self) -> Vec<SwordModule> {
        self.get_modules_by_category(vec!["Generic Books"])
    }
    pub fn get_map_modules(&self) -> Vec<SwordModule> {
        self.get_modules_by_category(vec!["Images", "Maps"])
    }

    // ------------------- INSTALL MODULE -------------------

    pub fn install_remote_module(&self, source: &str, module_name: &str) -> i32 {
        let c_source = CString::new(source).unwrap();
        let c_mod = CString::new(module_name).unwrap();

        PROGRESS_TOTAL.store(0, Ordering::SeqCst);
        PROGRESS_COMPLETED.store(0, Ordering::SeqCst);

        let path_str = self.sword_path.to_string_lossy().replace("\\", "/");
        let c_path = CString::new(path_str).unwrap();

        unsafe {
            // Create local, temporary handles for the installation.
            // This avoids blocking the main engine's Mutex during the long download/extract process,
            // allowing the user to keep reading their Bible while a module installs.
            let local_install_mgr = org_crosswire_sword_InstallMgr_new(c_path.as_ptr(), Some(Self::status_reporter));
            let local_mgr = org_crosswire_sword_SWMgr_newWithPath(c_path.as_ptr());

            println!(
                "[SwordEngine] Installing '{}' from '{}' (Background)",
                module_name, source
            );

            // 1. Refresh the source before installation
            org_crosswire_sword_InstallMgr_setUserDisclaimerConfirmed(local_install_mgr);
            org_crosswire_sword_InstallMgr_refreshRemoteSource(
                local_install_mgr,
                c_source.as_ptr(),
            );

            // 2. Sync the refreshed data
            org_crosswire_sword_InstallMgr_syncConfig(local_install_mgr);

            // 3. Now attempt installation using local handles
            let res = org_crosswire_sword_InstallMgr_remoteInstallModule(
                local_install_mgr,
                local_mgr,
                c_source.as_ptr(),
                c_mod.as_ptr(),
            );
            println!("[SwordEngine] Install result: {}", res);

            // 4. Cleanup local handles
            org_crosswire_sword_SWMgr_delete(local_mgr);
            org_crosswire_sword_InstallMgr_delete(local_install_mgr);

            // 5. If installation was successful, lock the main engine ONLY to rebuild it
            if res == 0 {
                println!("[SwordEngine] Installation successful, refreshing main engine awareness");
                let mut inner = self.inner.lock().unwrap();
                self.rebuild_mgr(&mut inner);
            }

            res
        }
    }

    pub fn get_download_progress(&self) -> f64 {
        let total = PROGRESS_TOTAL.load(Ordering::SeqCst);
        let completed = PROGRESS_COMPLETED.load(Ordering::SeqCst);
        if total == 0 {
            0.0
        } else {
            (completed as f64 / total as f64).clamp(0.0, 1.0)
        }
    }

    // ------------------- BIBLE STRUCTURE -------------------

    pub fn get_bible_structure(&self, module_name: &str) -> Vec<ModuleBook> {
        let mut books = Vec::new();
        let c_mod_name = CString::new(module_name).unwrap();
        let inner = self.inner.lock().unwrap();

        unsafe {
            let h_module =
                org_crosswire_sword_SWMgr_getModuleByName(inner.mgr, c_mod_name.as_ptr());
            if h_module == 0 {
                return books;
            }

            org_crosswire_sword_SWModule_begin(h_module);
            let mut current_book: Option<String> = None;
            let mut chapters = Vec::new();
            let mut current_chapter = 0;
            let mut verse_count = 0;

            loop {
                if org_crosswire_sword_SWModule_popError(h_module) != 0 {
                    break;
                }
                let key_ptr = org_crosswire_sword_SWModule_getKeyText(h_module);
                if key_ptr.is_null() {
                    break;
                }

                let key = CStr::from_ptr(key_ptr).to_string_lossy();
                let parts: Vec<&str> = key.split_whitespace().collect();
                if parts.len() < 2 {
                    org_crosswire_sword_SWModule_next(h_module);
                    continue;
                }

                let chap_part = parts.last().unwrap();
                let book_part = parts[..parts.len() - 1].join(" ");
                let chapter: i32 = chap_part
                    .split(':')
                    .next()
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(0);

                if current_book.as_deref() != Some(&book_part) {
                    if let Some(prev) = current_book.take() {
                        if verse_count > 0 {
                            chapters.push(ModuleChapter {
                                number: current_chapter,
                                verse_count,
                            });
                        }
                        books.push(ModuleBook {
                            name: prev,
                            chapters: chapters.clone(),
                        });
                    }
                    current_book = Some(book_part);
                    chapters.clear();
                    current_chapter = chapter;
                    verse_count = 0;
                }

                if chapter != current_chapter {
                    if verse_count > 0 {
                        chapters.push(ModuleChapter {
                            number: current_chapter,
                            verse_count,
                        });
                    }
                    current_chapter = chapter;
                    verse_count = 0;
                }
                verse_count += 1;
                org_crosswire_sword_SWModule_next(h_module);
            }
            if let Some(last) = current_book {
                if verse_count > 0 {
                    chapters.push(ModuleChapter {
                        number: current_chapter,
                        verse_count,
                    });
                }
                books.push(ModuleBook {
                    name: last,
                    chapters,
                });
            }
        }
        books
    }


    // ------------------- HELPERS -------------------

    fn ptr_to_str(&self, ptr: *const i8) -> String {
        if ptr.is_null() {
            "Unknown".to_string()
        } else {
            unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
        }
    }

    fn get_sword_path() -> PathBuf {
        let proj_dirs = ProjectDirs::from("org", "flame", "xbible").expect("Path error");
        let path = proj_dirs.data_local_dir().to_path_buf();
        fs::create_dir_all(&path).ok();
        path
    }

    fn prepare_app_directory(path: &PathBuf) {
        // 1. Create the fundamental SWORD structure
        let _ = fs::create_dir_all(path.join("mods.d"));
        let _ = fs::create_dir_all(path.join("modules"));

        // 2. CRITICAL: Create the specific folder the InstallMgr uses for Remote Sources
        // If this isn't here, the 'syncConfig' download has nowhere to land.
        let sources = ["CrossWire", "Bible.org", "IBT", "ebible.org"];
        for source in &sources {
            let remote_sources = path
                .join("InstallMgr")
                .join("RemoteSources")
                .join(source);
            let _ = fs::create_dir_all(&remote_sources);
        }

        let abs_path_str = path.to_string_lossy().replace("\\", "/");
        println!("absolute path: {}", abs_path_str);
        let conf_path = path.join("sword.conf");

        // Use the absolute path for DataPath.
        // We remove the #[wrap] logic here as per your permanent fix requirements.
        let config = format!(
            r#"[Globals]
DataPath={}
[Install]
Disclaimer=Confirmed
[Repos]
[Remote:CrossWire]
Description=CrossWire HTTP
Protocol=HTTP
Source=www.crosswire.org
Directory=/ftpmirror/pub/sword/raw
[Remote:Bible.org]
Description=Bible.org Repository
Protocol=HTTP
Source=ftp.bible.org
Directory=/sword
[Remote:IBT]
Description=Institute for Bible Translation
Protocol=HTTP
Source=ibt.org.ru
Directory=/sword
[Remote:ebible.org]
Description=eBible.org Repository
Protocol=HTTP
Source=ebible.org
Directory=/sword
"#,
            abs_path_str
        );

        if let Ok(mut file) = fs::File::create(conf_path) {
            let _ = writeln!(file, "{}", config);
        }
    }
}

impl Drop for SwordEngine {
    fn drop(&mut self) {
        let inner = self.inner.lock().unwrap();
        unsafe {
            println!("[SwordEngine] Cleaning up SWORD handles...");
            org_crosswire_sword_InstallMgr_delete(inner.install_mgr);
            org_crosswire_sword_SWMgr_delete(inner.mgr);
        }
    }
}
