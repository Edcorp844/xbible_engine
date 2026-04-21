use std::ffi::{CStr, CString};

use crate::{
    ffi::*, sword_engine::module_engine::sword_engine::SwordEngine};

#[derive(Debug, Clone)]
pub struct DictionaryResult {
    pub module_name: String,
    pub key: String,
    pub definition: String,
}

#[derive(Debug, Clone)]
pub struct DictionaryResponse {
    pub results: Vec<DictionaryResult>,
}

#[derive(Debug, Clone)]
pub struct DictionaryQuery {
    pub word: String,
    pub strongs: Vec<String>,
    pub language: String,
}

impl SwordEngine {
    pub fn lookup_dictionary(&self, query: DictionaryQuery) -> DictionaryResponse {
        let mut results = Vec::new();
        let dict_modules = self.get_dictionary_modules();

        let language_modules: Vec<_> = dict_modules
            .into_iter()
            .filter(|module| module.language.to_lowercase() == query.language.to_lowercase())
            .collect();

        for module in language_modules {
            let mut search_keys = Vec::new();
            if !query.word.is_empty() {
                search_keys.push(query.word.clone());
            }

            for key in search_keys {
                // We try the key variations
                let keys_to_try = vec![key.clone(), key.to_uppercase(), key.to_lowercase()];

                for k in keys_to_try {
                    // 1. Attempt to get the entry
                    if let Some((actual_key, definition)) =
                        self.get_dictionary_entry_with_key_check(&module.name, &k)
                    {
                        // 2. STRICT CHECK: Does the actual key from SWORD match our requested key?
                        // We use case-insensitive comparison to be safe, or exact if you prefer.
                        if actual_key.to_lowercase() == k.to_lowercase() {
                            results.push(DictionaryResult {
                                module_name: module.description.clone(),
                                key: actual_key, // Use the official key from the module
                                definition: definition,
                            });
                            break; // Found the exact match for this module
                        }
                    }
                }
            }
        }
        DictionaryResponse { results }
    }

    fn get_dictionary_entry_with_key_check(
        &self,
        module_name: &str,
        key: &str,
    ) -> Option<(String, String)> {
        let inner = self.inner.lock().unwrap();
        unsafe {
            let c_mod = CString::new(module_name).ok()?;
            let c_key = CString::new(key).ok()?;
            let h_mgr = inner.mgr;
            let h_mod = org_crosswire_sword_SWMgr_getModuleByName(h_mgr, c_mod.as_ptr());

            if h_mod == 0 {
                return None;
            }

            // Set the key
            org_crosswire_sword_SWModule_setKeyText(h_mod, c_key.as_ptr());

            // Check for SWORD errors (Key not found)
            if org_crosswire_sword_SWModule_popError(h_mod) != 0 {
                return None;
            }

            // 3. GET ACTUAL KEY: See where SWORD actually landed
            let actual_key_ptr = org_crosswire_sword_SWModule_getKeyText(h_mod);
            if actual_key_ptr.is_null() {
                return None;
            }
            let actual_key = CStr::from_ptr(actual_key_ptr)
                .to_string_lossy()
                .into_owned();

            // 4. GET RENDERED TEXT
            let text_ptr = org_crosswire_sword_SWModule_renderText(h_mod);
            if text_ptr.is_null() {
                return None;
            }
            let text = CStr::from_ptr(text_ptr).to_string_lossy().into_owned();

            if text.trim().is_empty() {
                None
            } else {
                Some((actual_key, text))
            }
        }
    }

    fn get_dictionary_entry_direct(&self, module_name: &str, key: &str) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        unsafe {
            let c_mod = CString::new(module_name).ok()?;
            let c_key = CString::new(key).ok()?;
            let h_mgr = inner.mgr;
            let h_mod = org_crosswire_sword_SWMgr_getModuleByName(h_mgr, c_mod.as_ptr());

            if h_mod == 0 {
                return None;
            }

            org_crosswire_sword_SWModule_setKeyText(h_mod, c_key.as_ptr());
            if org_crosswire_sword_SWModule_popError(h_mod) != 0 {
                return None;
            }

            let text_ptr = org_crosswire_sword_SWModule_renderText(h_mod);
            if text_ptr.is_null() {
                return None;
            }

            let text = CStr::from_ptr(text_ptr).to_string_lossy().into_owned();
            if text.trim().is_empty() {
                None
            } else {
                Some(text)
            }
        }
    }

    
}
