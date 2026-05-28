use std::ffi::{CStr, CString};
use crate::{engines::module_engine::module_engine::ModuleEngine, ffi::*};

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

impl ModuleEngine {
    pub fn lookup_strongs_number(&self, query: LexiconQuery) -> LexiconResponse {
        let mut results = Vec::new();

        let raw_target = query.strongs_number.trim().to_uppercase();
        if raw_target.is_empty() {
            return LexiconResponse { results };
        }

        let is_greek_query = raw_target.starts_with('G') || query.language.eq_ignore_ascii_case("Greek");
        let is_hebrew_query = raw_target.starts_with('H') || query.language.eq_ignore_ascii_case("Hebrew");

        let digit_chars: String = raw_target.chars().filter(|c| c.is_ascii_digit()).collect();
        if digit_chars.is_empty() {
            return LexiconResponse { results };
        }

        let strongs_int: u32 = match digit_chars.parse() {
            Ok(num) => num,
            Err(_) => return LexiconResponse { results },
        };

        let numeric_part = strongs_int.to_string();
        let dict_modules = self.get_dictionary_modules();

        for module in dict_modules {
            if !self.is_legitimate_strongs_lexicon(&module.name, &module.description) {
                continue;
            }

            let module_name_lower = module.name.to_lowercase();
            
            // Only filter out modules if we're certain about the language mismatch
            // For example, explicitly skip "StrongsGreek" for Hebrew queries
            let is_explicitly_greek = module_name_lower.contains("greek");
            let is_explicitly_hebrew = module_name_lower.contains("hebrew");
            
            if is_greek_query && is_explicitly_hebrew {
                continue;
            }
            if is_hebrew_query && is_explicitly_greek {
                continue;
            }

            let mut key_variants = Vec::new();
            let prefix = if is_greek_query { "G" } else { "H" };

            // Try prefixed variants first (primary lookup keys)
            key_variants.push(format!("{}{}", prefix, numeric_part));             
            key_variants.push(format!("{}{:0>4}", prefix, numeric_part));         
            key_variants.push(format!("{}{:0>5}", prefix, numeric_part));
            
            // Add unprefixed variants as fallback for modules that don't use prefixes
            key_variants.push(format!("{:0>4}", numeric_part));                   
            key_variants.push(format!("{:0>5}", numeric_part));                   
            key_variants.push(numeric_part.clone());                              

            let mut seen_variants = std::collections::HashSet::new();
            key_variants.retain(|v| seen_variants.insert(v.clone()));

            let mut found_match = false;

            for variant in &key_variants {
                if let Some((resolved_key, definition)) =
                    self.get_lexicon_entry_raw(&module.name, variant)
                {
                    let clean_def = self.sanitize_definition_text(&definition);
                    if clean_def.is_empty() || clean_def.starts_with("@LINK") || clean_def.contains("@@@@") {
                        continue;
                    }

                    if !results.iter().any(|r| r.module_name == module.description) {
                        results.push(LexiconResult {
                            module_name: module.description.clone(),
                            key: resolved_key.clone(),
                            definition: clean_def,
                        });
                    }
                    found_match = true;
                    break;
                }
            }

            if !found_match {
                if let Some((resolved_key, definition)) = self.find_lexicon_entry_by_iteration(&module.name, &numeric_part, prefix) {
                    let clean_def = self.sanitize_definition_text(&definition);
                    if !results.iter().any(|r| r.module_name == module.description) && !clean_def.is_empty() {
                        results.push(LexiconResult {
                            module_name: module.description.clone(),
                            key: resolved_key.clone(),
                            definition: clean_def,
                        });
                    }
                }
            }
        }

        LexiconResponse { results }
    }

    fn is_legitimate_strongs_lexicon(&self, name: &str, description: &str) -> bool {
        let name_lower = name.to_lowercase();
        let desc_lower = description.to_lowercase();

        if name_lower.contains("strong") || desc_lower.contains("strong") || name_lower.contains("lexicon") || desc_lower.contains("lexicon") || name_lower.contains("dodson") {
            if name_lower.contains("webster") 
                || name_lower.contains("easton") 
                || name_lower.contains("tract") 
                || name_lower.contains("hitchcock")
                || name_lower.contains("international standard")
                || name_lower.contains("net bible concordance") 
            {
                return false;
            }
            return true;
        }
        false
    }

    // High performance markup and structural symbol sanitation engine
    fn sanitize_definition_text(&self, raw_html: &str) -> String {
        let mut output = String::with_capacity(raw_html.len());
        let mut in_tag = false;
        
        // Convert break markers to standard newlines before removing generic tags
        let pre_processed = raw_html.replace("<br />", "\n").replace("<br/>", "\n").replace("<br>", "\n");

        for c in pre_processed.chars() {
            match c {
                '<' => in_tag = true,
                '>' => {
                    in_tag = false;
                    continue;
                }
                _ => {
                    if !in_tag {
                        output.push(c);
                    }
                }
            }
        }

        // Clean up formatting leakage, sequential whitespace blocks, and unicode accents remnants
        let mut lines: Vec<String> = output
            .lines()
            .map(|line| {
                line.replace('\u{301}', "") // Clear accidental accent combining marks
                    .replace("&nbsp;", " ")
                    .trim()
                    .to_string()
            })
            .filter(|line| !line.is_empty())
            .collect();

        // Strip leading token numbers if redundantly present at index header boundaries
        if Option::is_some(&lines.first().map(|l| l.chars().all(|c| c.is_ascii_digit() || c == 'G' || c == 'H'))) {
            lines.remove(0);
        }

        lines.join("\n").trim().to_string()
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

            let resolved_key = CStr::from_ptr(actual_key_ptr).to_string_lossy().into_owned();
            let clean_key = key.trim().to_uppercase();
            let clean_resolved = resolved_key.trim().to_uppercase();

            if clean_resolved.is_empty() {
                return None;
            }

            // For Strong's numbers, validate more strictly
            // Extract alphabetic prefixes and numeric parts
            let key_prefix = clean_key.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>();
            let resolved_prefix = clean_resolved.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>();
            
            let key_digits: String = clean_key.chars().filter(|c| c.is_ascii_digit()).collect();
            let resolved_digits: String = clean_resolved.chars().filter(|c| c.is_ascii_digit()).collect();
            
            // If we're searching for a prefixed key (like "H2617"):
            if !key_prefix.is_empty() && !key_digits.is_empty() {
                // If resolved key has a prefix, it must match
                if !resolved_prefix.is_empty() && key_prefix != resolved_prefix {
                    return None;
                }
                
                // Numeric parts must match
                if !resolved_digits.is_empty() {
                    let key_num = key_digits.trim_start_matches('0').parse::<u32>().unwrap_or(0);
                    let resolved_num = resolved_digits.trim_start_matches('0').parse::<u32>().unwrap_or(0);
                    if key_num > 0 && resolved_num > 0 && key_num != resolved_num {
                        return None;
                    }
                }
            }

            if clean_key != clean_resolved && !clean_resolved.contains(&clean_key) && !clean_key.contains(&clean_resolved) {
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

    fn find_lexicon_entry_by_iteration(&self, module_name: &str, numeric_target: &str, expected_prefix: &str) -> Option<(String, String)> {
        let inner = self.inner.lock().unwrap();
        unsafe {
            let c_mod = CString::new(module_name).ok()?;
            let h_mgr = inner.mgr;
            let h_mod = org_crosswire_sword_SWMgr_getModuleByName(h_mgr, c_mod.as_ptr());

            if h_mod == 0 { return None; }

            org_crosswire_sword_SWModule_setKeyText(h_mod, CString::new("00001").ok()?.as_ptr());
            
            let mut steps = 0;
            while steps < 5500 {
                let key_ptr = org_crosswire_sword_SWModule_getKeyText(h_mod);
                if key_ptr.is_null() { break; }

                let resolved_key = CStr::from_ptr(key_ptr).to_string_lossy().into_owned();
                let clean_resolved = resolved_key.trim().to_uppercase();

                let resolved_digits: String = clean_resolved.chars().filter(|c| c.is_ascii_digit()).collect();
                let normalized_resolved_digits = resolved_digits.parse::<u32>().map(|n| n.to_string()).unwrap_or_default();
                // Check prefix matches and numeric portion matches
                let prefix_matches = if let Some(first_char) = clean_resolved.chars().next() {
                    first_char.to_string().to_uppercase() == expected_prefix.to_uppercase()
                } else {
                    false
                };

                if prefix_matches && normalized_resolved_digits == numeric_target {
                    let text_ptr = org_crosswire_sword_SWModule_renderText(h_mod);
                    if !text_ptr.is_null() {
                        let definition = CStr::from_ptr(text_ptr).to_string_lossy().into_owned();
                        if !definition.trim().is_empty() {
                            return Some((resolved_key, definition));
                        }
                    }
                }

                org_crosswire_sword_SWModule_next(h_mod);
                if org_crosswire_sword_SWModule_popError(h_mod) != 0 {
                    break;
                }
                steps += 1;
            }
            None
        }
    }
}