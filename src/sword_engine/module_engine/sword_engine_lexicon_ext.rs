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
    pub requested_key: String,
    pub resolved_key: String,
    pub is_exact_match: bool,
    pub definition: String,
    pub lemma: String,
    pub gloss: Option<String>,
    pub morph: Vec<String>,
    pub notes: Vec<String>,
    pub related_words: Vec<String>,
    pub concordance_entries: Vec<String>,
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

        println!("\n==================================================");
        println!("=== [DEBUG] LEXICON ENGINE LOOKUP START ===");
        println!(
            "Target Key: '{}' | Requested Language: '{}'",
            target_key, query.language
        );
        println!("==================================================");

        if target_key.is_empty() {
            return LexiconResponse { results };
        }

        let target_numeric = target_key
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>();
        let target_u32 = target_numeric.parse::<u32>().ok();
        let target_prefix = target_key.chars().find(|c| c.is_ascii_alphabetic());

        let raw_tag_variant1 = format!("class=\"entryFree\">{}", target_key);
        let raw_tag_variant2 = format!("class=\"entryFree\">{}", target_numeric);

        let language_modules: Vec<_> = dict_modules
            .into_iter()
            .filter(|module| module.language.to_lowercase() == query.language.to_lowercase())
            .collect();

        for module in language_modules {
            // Skip default reference dictionaries that cause fallback noise
            if module.name.starts_with("Webster") {
                continue;
            }

            let keys_to_try = self.get_strongs_variants(&target_key);
            println!("\n  ⚡ Processing Module: '{}'", module.name);

            for key_variant in keys_to_try {
                if let Some((resolved_key, mut definition)) =
                    self.get_lexicon_entry_raw(&module.name, &key_variant)
                {
                    let clean_resolved = resolved_key.trim().to_uppercase();

                    // --- 1. DETECT STRUCTURAL TAG MATCHES ---
                    let tag1_match = definition.contains(&raw_tag_variant1);
                    let tag2_match = definition.contains(&raw_tag_variant2);
                    let bracket_match = !target_numeric.is_empty()
                        && definition.contains(&format!(">{}</", target_numeric));
                    let strong_found_in_text = tag1_match || tag2_match || bracket_match;

                    if strong_found_in_text {
                        println!(
                            "       🚀 [PASSED OVERRIDE] Verified via raw markup stream tag content."
                        );

                        // Sanitize internal layout links to prevent UI rendering crashes
                        if definition.contains("sword://") {
                            definition = definition.replace("sword://", "lexicon://");
                        }

                        let final_key = if clean_resolved.is_empty() {
                            key_variant.clone()
                        } else {
                            resolved_key
                        };
                        results.push(LexiconResult {
                            module_name: module.description.clone(),
                            requested_key: target_key.clone(),
                            resolved_key: final_key,
                            is_exact_match: true,
                            definition,
                            lemma: String::new(),
                            gloss: None,
                            morph: Vec::new(),
                            notes: Vec::new(),
                            related_words: Vec::new(),
                            concordance_entries: Vec::new(),
                        });
                        break;
                    }

                    // --- 2. FALLBACK METRIC CHECK (ANTI-HIJACK GUARD) ---
                    let resolved_numeric = clean_resolved
                        .chars()
                        .filter(|c| c.is_ascii_digit())
                        .collect::<String>();
                    let resolved_u32 = resolved_numeric.parse::<u32>().ok();

                    // If the engine returns a non-numeric text string (like 'ἌΑΠΤΟΣ') for a Strong's lookup,
                    // and it didn't pass our text scan target verification above, reject it completely!
                    if resolved_numeric.is_empty() {
                        println!(
                            "       ❌ [REJECTED HIJACK] Lexicon match returned textual dictionary word layout node '{}' instead of a Strong's index reference.",
                            clean_resolved
                        );
                        continue;
                    }

                    if clean_resolved.len() <= 2 && resolved_numeric.is_empty() {
                        continue;
                    }

                    if let (Some(t_num), Some(r_num)) = (target_u32, resolved_u32) {
                        if t_num != r_num {
                            continue;
                        }
                    }

                    let resolved_prefix = clean_resolved.chars().find(|c| c.is_ascii_alphabetic());
                    if let (Some(t_pref), Some(r_pref)) = (target_prefix, resolved_prefix) {
                        if t_pref != r_pref {
                            continue;
                        }
                    }

                    let is_exact = clean_resolved == target_key
                        || clean_resolved.contains(&target_key)
                        || target_key.contains(&clean_resolved);

                    if definition.contains("sword://") {
                        definition = definition.replace("sword://", "lexicon://");
                    }

                    println!("       ✅ [PASSED METRICS] Standard structural fallback validated.");
                    results.push(LexiconResult {
                        module_name: module.description.clone(),
                        requested_key: target_key.clone(),
                        resolved_key,
                        is_exact_match: is_exact,
                        definition,
                        lemma: String::new(),
                        gloss: None,
                        morph: Vec::new(),
                        notes: Vec::new(),
                        related_words: Vec::new(),
                        concordance_entries: Vec::new(),
                    });

                    break;
                }
            }
        }

        println!(
            "=== [DEBUG] LEXICON ENGINE LOOKUP END | Count: {} ===",
            results.len()
        );
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

    fn get_strongs_variants(&self, clean_key: &str) -> Vec<String> {
        let mut variants = vec![clean_key.to_string()];

        let mut chars = clean_key.chars();
        if let Some(prefix) = chars.next() {
            let remainder: String = chars.collect();
            if let Ok(num) = remainder.parse::<u32>() {
                variants.push(format!("{}{:04}", prefix, num));
                variants.push(num.to_string());
                variants.push(format!("{}{:05}", prefix, num));
            }
        }
        variants
    }
}