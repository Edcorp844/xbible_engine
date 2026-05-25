use std::ffi::{CStr, CString};

use crate::{ffi::*, sword_engine::SwordEngine};

#[derive(Debug, Clone, uniffi::Record)]
pub struct LexiconQuery {
    pub strongs_number: String,
    pub language: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct LexiconResult {
    pub module_name: String,
    pub key: String,
    pub definition: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct LexiconResponse {
    pub results: Vec<LexiconResult>,
}

impl SwordEngine {
                   
         pub fn lookup_strongs_number(&self, query: LexiconQuery) -> LexiconResponse {
        let mut results = Vec::new();

        let dict_modules = self.get_dictionary_modules();
        let target_key = query.strongs_number.trim().to_uppercase();

        if target_key.is_empty() {
            return LexiconResponse { results };
        }

        let target_numeric: String = target_key.chars().filter(|c| c.is_ascii_digit()).collect();
        let target_prefix = target_key.chars().find(|c| c.is_ascii_alphabetic());

        let language_modules: Vec<_> = dict_modules
            .into_iter()
            .filter(|module| module.language.eq_ignore_ascii_case(&query.language))
            .collect();

        for module in language_modules {
            if module.name.starts_with("Webster") {
                continue;
            }

            // Search directly using the exact target_key as it is
            if let Some((resolved_key, mut definition)) =
                self.get_lexicon_entry_raw(&module.name, &target_key)
            {
                let clean_resolved = resolved_key.trim().to_uppercase();

                let resolved_numeric: String =
                    clean_resolved.chars().filter(|c| c.is_ascii_digit()).collect();

                if resolved_numeric.is_empty() {
                    continue;
                }

                if let (Some(t_num), Some(r_num)) =
                    (target_numeric.parse::<u32>().ok(), resolved_numeric.parse::<u32>().ok())
                {
                    if t_num != r_num {
                        continue;
                    }
                }

                if let (Some(t_pref), Some(r_pref)) = (target_prefix, clean_resolved.chars().find(|c| c.is_ascii_alphabetic())) {
                    if t_pref != r_pref {
                        continue;
                    }
                }

                if definition.contains("sword://") {
                    definition = definition.replace("sword://", "lexicon://");
                }

                results.push(LexiconResult {
                    module_name: module.description.clone(),
                    key: target_key.clone(),
                    definition,
                });

                // --- REMOVED BREAK STATEMENT ---
                // Removing 'break;' ensures the engine keeps iterating 
                // through all remaining dictionary modules instead of stopping.
            }
        }

        LexiconResponse { results }
    }

   

    fn get_lexicon_entry_raw(&self, module_name: &str, key: &str) -> Option<(String, String)> {
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

            let actual_key_ptr = org_crosswire_sword_SWModule_getKeyText(h_mod);
            if actual_key_ptr.is_null() {
                return None;
            }
            let resolved_key = CStr::from_ptr(actual_key_ptr)
                .to_string_lossy()
                .into_owned();

            // --- INDEX MISMATCH OVERFLOW GUARD ---
            // If the key requested doesn't structurally align with the resolved key,
            // and the module snapped back to index 0 (or didn't change variants), dump it early.
            let clean_key = key.trim().to_uppercase();
            let clean_resolved = resolved_key.trim().to_uppercase();
            
            if clean_key != clean_resolved && !clean_resolved.contains(&clean_key) && !clean_key.contains(&clean_resolved) {
                // If the resolved key is completely alphabetic text (like 'ἌΑΠΤΟΣ') but we searched for a numeric 
                // Strong's variation key, SWORD defaulted to entry zero. Reject it here before rendering layout streams.
                let key_has_digits = clean_key.chars().any(|c| c.is_ascii_digit());
                let resolved_has_digits = clean_resolved.chars().any(|c| c.is_ascii_digit());
                
                if key_has_digits && !resolved_has_digits {
                    return None;
                }
            }

            let text_ptr = org_crosswire_sword_SWModule_renderText(h_mod);
            if text_ptr.is_null() {
                return None;
            }
            let definition = CStr::from_ptr(text_ptr).to_string_lossy().into_owned();

            if definition.trim().is_empty() {
                None
            } else {
                Some((resolved_key, definition))
            }
        }
    }

}