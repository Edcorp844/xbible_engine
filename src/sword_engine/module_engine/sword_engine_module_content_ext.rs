use serde::{Deserialize, Serialize};
use std::ffi::CString;

use crate::sword_engine::{module_engine::{sword_engine::SwordEngine, sword_module::SwordModule}, osis_translation_engine::engine::OsisTransilationEngine};
use crate::ffi::*;


// --- DATA STRUCTURES ---

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[derive(uniffi::Record)]
pub struct LexicalInfo {
    pub strongs: Vec<String>,
    pub lemma: Option<String>,
    pub gloss: Option<String>,
    pub morph: Vec<String>,
}

#[derive(Debug, Clone)]
#[derive(uniffi::Record)]
pub struct Word {
    pub text: String,
    pub is_red: bool,
    pub is_italic: bool,
    pub is_bold_text: bool,
    pub lex: Option<LexicalInfo>,
    pub note: Option<String>,
    pub is_first_in_group: bool,
    pub is_last_in_group: bool,
    pub is_punctuation: bool,
    pub is_title: bool,
    pub language: String,
}

impl Default for Word {
    fn default() -> Self {
        Self {
            text: String::new(),
            lex: None,
            is_red: false,
            is_italic: false,
            is_bold_text: false,
            is_punctuation: false,
            is_first_in_group: false,
            is_last_in_group: false,
            is_title: false,
            language: String::new(),
            note: None,
        }
    }
}

#[derive(Debug, Clone)]
#[derive(uniffi::Record)]
pub struct Verse {
    pub osis_id: String,
    pub number: i32,
    pub words: Vec<Word>,
    pub notes: Vec<String>,
    pub is_paragraph_start: bool,
}

#[derive(Debug, Clone, PartialEq)]
#[derive(uniffi::Enum)]
pub enum TextDirection {
    Rtl,
    Ltr,
}


#[derive(Debug, Clone)]
#[derive(uniffi::Record)]
pub struct Section {
    pub title: Vec<Word>,
    pub verses: Vec<Verse>,
    pub text_direction: TextDirection,
}

// --- CORE ENGINE IMPLEMENTATION ---

impl SwordEngine {
    unsafe fn fetch_and_parse_current_entry(
        &self,
        h_mod: isize,
        module: &SwordModule,
        osis_engine: &OsisTransilationEngine,
    ) -> Vec<Section> {
        let current_key =
            unsafe { self.sword_ptr_to_string(org_crosswire_sword_SWModule_getKeyText(h_mod)) };

        if let Some(raw_osis) =
            unsafe { self.sword_ptr_to_string(org_crosswire_sword_SWModule_getRawEntry(h_mod)) }
        {
            //println!("[SINGLE ENTRY RAW]: {}", raw_osis);
            return osis_engine.parse_osis_to_sections(
                module.language.clone(),
                &raw_osis,
                current_key,
            );
        }
        Vec::new()
    }

    pub fn get_single_entry(&self, module: Option<&SwordModule>, reference: &str) -> Vec<Section> {
        let osis_engine = OsisTransilationEngine::new();
        let resolved_module = match module {
            Some(m) => m.clone(),
            None => match self
                .get_modules()
                .into_iter()
                .find(|m| m.category == "Biblical Texts")
            {
                Some(m) => m,
                None => return Vec::new(),
            },
        };

        unsafe {
            let inner = self.inner.lock().unwrap();
            let mod_name = CString::new(resolved_module.name.as_str()).unwrap();
            let h_mod = org_crosswire_sword_SWMgr_getModuleByName(inner.mgr, mod_name.as_ptr());

            if h_mod == 0 {
                return Vec::new();
            }

            let c_ref = CString::new(reference).unwrap();
            org_crosswire_sword_SWModule_setKeyText(h_mod, c_ref.as_ptr());

            self.fetch_and_parse_current_entry(h_mod, &resolved_module, &osis_engine)
        }
    }

    pub fn get_whole_chapter(&self, module: &SwordModule, reference: &str) -> Vec<Section> {
        let mut raw_entries = Vec::new();

        let inner = self.inner.lock().unwrap();
        let mod_name = CString::new(module.name.as_str()).unwrap();
        let h_mod =
            unsafe { org_crosswire_sword_SWMgr_getModuleByName(inner.mgr, mod_name.as_ptr()) };

        if h_mod == 0 {
            eprintln!("[SWORD ERROR]: Module '{}' not found!", module.name);
            return Vec::new();
        }

        let opt_name = CString::new("Headings").unwrap();
        let opt_on = CString::new("On").unwrap();
        unsafe {
            org_crosswire_sword_SWMgr_setGlobalOption(inner.mgr, opt_name.as_ptr(), opt_on.as_ptr())
        };

        let c_ref = CString::new(reference).unwrap();
        unsafe { org_crosswire_sword_SWModule_setKeyText(h_mod, c_ref.as_ptr()) };

        let initial_key =
            unsafe { self.sword_ptr_to_string(org_crosswire_sword_SWModule_getKeyText(h_mod)) }
                .unwrap_or_else(|| "Unknown".to_string());

        let (target_book, target_chapter) = match self.parse_reference(&initial_key) {
            Some(val) => val,
            None => return Vec::new(),
        };

        println!(
            "\n--- [DUMPING RAW OSIS FOR {} {}] ---",
            target_book, target_chapter
        );

        let v1_key = format!("{} {}:1", target_book, target_chapter);

        let c_v1 = CString::new(v1_key.as_str()).unwrap();
        unsafe { org_crosswire_sword_SWModule_setKeyText(h_mod, c_v1.as_ptr()) };

        loop {
            let current_key =
                unsafe { self.sword_ptr_to_string(org_crosswire_sword_SWModule_getKeyText(h_mod)) }
                    .unwrap_or_else(|| "Unknown".to_string());

            let (curr_book, curr_chap) = match self.parse_reference(&current_key) {
                Some(val) => val,
                None => break,
            };

            if curr_book != target_book || curr_chap != target_chapter {
                break;
            }

            if let Some(raw_xml) =
                unsafe { self.sword_ptr_to_string(org_crosswire_sword_SWModule_getRawEntry(h_mod)) }
            {
                println!("[KEY: {}] RAW OSIS: {}", current_key, raw_xml);
                raw_entries.push((current_key, raw_xml));
            }

            unsafe { org_crosswire_sword_SWModule_next(h_mod) };
            if unsafe { org_crosswire_sword_SWModule_popError(h_mod) } != 0 {
                break;
            }
        }

        let engine = OsisTransilationEngine::new();
        engine.parse_osis_list_to_sections(module.language.clone(), raw_entries)
    }

    fn parse_reference(&self, full_key: &str) -> Option<(String, String)> {
        let last_space_idx = full_key.rfind(' ')?;
        let book = full_key[..last_space_idx].to_lowercase();
        let rest = &full_key[last_space_idx + 1..];
        let chapter = match rest.find(':') {
            Some(idx) => &rest[..idx],
            None => rest,
        };
        Some((book, chapter.to_string()))
    }
}
