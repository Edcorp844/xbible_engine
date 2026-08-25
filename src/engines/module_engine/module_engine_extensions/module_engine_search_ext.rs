use crate::engines::module_engine::module_engine::ModuleEngine;
use crate::ffi;
use std::ffi::{CStr, CString};

#[derive(Debug, Clone, uniffi::Record)]
pub struct SearchHit {
    pub module_name: String,
    pub key: String,
    pub score: i64,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct SearchResults {
    pub query: String,
    pub module_name: String,
    pub hits: Vec<SearchHit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SearchType {
    RegularExpression = 1,
    Phrase = -1,
    MultiWord = -2,
}

impl SearchType {
    pub fn to_i32(&self) -> i32 {
        match self {
            SearchType::RegularExpression => 1,
            SearchType::Phrase => -1,
            SearchType::MultiWord => -2,
        }
    }
}

unsafe extern "C" fn noop_search_callback(_percent: ::std::os::raw::c_int) {}

impl ModuleEngine {
    pub fn search(
        &self,
        module_name: String,
        query: String,
        search_type: SearchType,
    ) -> SearchResults {
        let inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(_) => return SearchResults::default(),
        };

        if inner.mgr == 0 {
            return SearchResults::default();
        }

        let c_module_name = match CString::new(module_name.clone()) {
            Ok(s) => s,
            Err(_) => return SearchResults::default(),
        };

        let c_query = match CString::new(query.clone()) {
            Ok(s) => s,
            Err(_) => return SearchResults::default(),
        };

        unsafe {
            let h_module =
                ffi::org_crosswire_sword_SWMgr_getModuleByName(inner.mgr, c_module_name.as_ptr());

            if h_module == 0 {
                log::error!("[Search] Module '{}' not found", module_name);
                return SearchResults::default();
            }

            // Provide a valid non-null callback function pointer to prevent flatapi SEGV
            let hits_ptr = ffi::org_crosswire_sword_SWModule_search(
                h_module,
                c_query.as_ptr(),
                search_type.to_i32(),
                0,                  // flags
                std::ptr::null(),   // search scope (NULL searches full module)
                Some(noop_search_callback),
            );

            let mut results = SearchResults {
                query,
                module_name,
                hits: Vec::new(),
            };

            if !hits_ptr.is_null() {
                let mut current_ptr = hits_ptr;

                // Iterate over SWORD's null-terminated array of SearchHit structs
                while !current_ptr.is_null() && !(*current_ptr).key.is_null() {
                    let hit = &*current_ptr;

                    let mod_name = if !hit.modName.is_null() {
                        CStr::from_ptr(hit.modName)
                            .to_string_lossy()
                            .into_owned()
                    } else {
                        String::new()
                    };

                    let key = CStr::from_ptr(hit.key)
                        .to_string_lossy()
                        .into_owned();

                    results.hits.push(SearchHit {
                        module_name: mod_name,
                        key,
                        score: hit.score as i64,
                    });

                    current_ptr = current_ptr.add(1);
                }
            }

            results
        }
    }
}