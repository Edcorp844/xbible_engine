use roxmltree::Node;

use crate::engines::{
    module_engine::module_engine_extensions::module_engine_module_content_ext::{
        LexicalInfo, TextDirection, Verse, Word,
    },
    osis_translation_engine::engine::OsisTransilationEngine,
};

impl OsisTransilationEngine {
    pub(crate) fn create_word(
        &self,
        text: String,
        is_added: bool,
        is_red: bool,
        is_italic: bool,
        is_inside_title: bool,
        is_divine: bool,
        lex: Option<&LexicalInfo>,
        language: &str,
    ) -> Word {
        let is_punct = text
            .chars()
            .all(|c| c.is_ascii_punctuation() || ('\u{3000}'..='\u{303F}').contains(&c));

        Word {
            text,
            is_red,
            is_italic,
            is_bold_text: is_inside_title || is_divine,
            is_added,
            is_first_added: false,
            is_last_added: false,
            lex: lex.cloned(),
            note: None,
            is_first_in_group: false,
            is_last_in_group: false,
            is_title: is_inside_title,
            is_punctuation: is_punct,
            language: language.to_string(),
        }
    }

    pub(crate) fn is_non_segmented(&self, text: &str) -> bool {
        text.chars().any(|c| {
            ('\u{4E00}'..='\u{9FFF}').contains(&c) || ('\u{3040}'..='\u{30FF}').contains(&c)
        })
    }

    pub fn collect_note_text(&self, node: Node) -> String {
        let mut buf = String::new();
        for child in node.descendants() {
            if child.is_text() {
                if let Some(t) = child.text() {
                    let trimmed = t.trim();
                    if !trimmed.is_empty() {
                        if !buf.is_empty() && !buf.ends_with(' ') {
                            buf.push(' ');
                        }
                        buf.push_str(trimmed);
                    }
                }
            }
        }
        buf
    }

    pub(crate) fn apply_group_metadata(&self, words: &mut [Word]) {
        let len = words.len();
        for i in 0..len {
            if words[i].is_red {
                let prev = i > 0 && words[i - 1].is_red;
                let next = i < len - 1 && words[i + 1].is_red;
                words[i].is_first_in_group = !prev;
                words[i].is_last_in_group = !next;
            }
        }
    }

    pub(crate) fn extract_verse_number(&self, key: &str) -> i32 {
        key.split(|c| c == '.' || c == ':')
            .last()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    pub(crate) fn detect_direction(&self, verse: &Verse) -> TextDirection {
        let sample = verse.words.first().map(|w| w.text.as_str()).unwrap_or("");
        let is_rtl = sample.chars().any(|c| {
            ('\u{0600}'..='\u{06FF}').contains(&c)
                || ('\u{0750}'..='\u{077F}').contains(&c)
                || ('\u{0590}'..='\u{05FF}').contains(&c)
        });
        if is_rtl {
            TextDirection::Rtl
        } else {
            TextDirection::Ltr
        }
    }
}
