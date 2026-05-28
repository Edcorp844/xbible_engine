use roxmltree::Node;

use crate::engines::{ module_engine::module_engine_extensions::module_engine_module_content_ext::{LexicalInfo, Word}, osis_translation_engine::engine::OsisTransilationEngine};


impl OsisTransilationEngine{
      pub(crate) fn parse_osis_content(
        &self,
        language: &str,
        root: Node,
    ) -> (Vec<Word>, Vec<String>, Option<Vec<Word>>) {
        let mut words = Vec::with_capacity(64);
        let mut verse_notes = Vec::new();
        let mut title_words = Vec::new();

        self.walk_osis(
            root,
            &mut words,
            &mut verse_notes,
            &mut title_words,
            None,
            false,
            false,
            false,
            false,
            false,
            false,
            language,
        );

        let final_title = if title_words.is_empty() {
            None
        } else {
            Some(title_words)
        };
        (words, verse_notes, final_title)
    }

    fn walk_osis(
        &self,
        node: Node,
        words: &mut Vec<Word>,
        verse_notes: &mut Vec<String>,
        title_accumulator: &mut Vec<Word>,
        parent_lex: Option<&LexicalInfo>,
        is_red: bool,
        is_added: bool,
        is_italic: bool,
        is_inside_title: bool,
        is_inside_note: bool,
        is_divine: bool,
        language: &str,
    ) {
        if node.is_element() {
            let mut active_lex_owned: Option<LexicalInfo> = None;
            let mut active_red = is_red;
            let mut active_added = is_added;
            let mut active_italic = is_italic;
            let mut active_divine = is_divine;
            let mut traversing_title = is_inside_title;
            let mut _traversing_note = is_inside_note;

            if node.has_tag_name("title") {
                traversing_title = true;
            } else if node.has_tag_name("w") {
                if let Some(raw_lemma) = node.attribute("lemma") {
                    active_lex_owned = Some(LexicalInfo {
                        strongs: raw_lemma
                            .split_whitespace()
                            .filter(|s| s.starts_with("strong:"))
                            .map(|s| s.trim_start_matches("strong:").to_string())
                            .collect(),
                        ..Default::default()
                    });
                }
            } else if node.has_tag_name("divineName") {
                active_divine = true;
            } else if node.has_tag_name("q") && node.attribute("who") == Some("Jesus") {
                active_red = true;
            } else if node.has_tag_name("transChange") && node.attribute("type") == Some("added") {
                active_added = true;
            } else if node.has_tag_name("hi") && node.attribute("type") == Some("italic") {
                active_italic = true;
            } else if node.has_tag_name("note") {
                _traversing_note = true;
                let text = self.collect_note_text(node);
                if !text.is_empty() {
                    verse_notes.push(text);
                }
                return;
            }

            let lex_to_pass = active_lex_owned.as_ref().or(parent_lex);
            for child in node.children() {
                self.walk_osis(
                    child,
                    words,
                    verse_notes,
                    title_accumulator,
                    lex_to_pass,
                    active_red,
                    active_added,
                    active_italic,
                    traversing_title,
                    _traversing_note,
                    active_divine,
                    language,
                );
            }
        } else if node.is_text() {
            if is_inside_note {
                return;
            }
            let text = node.text().unwrap_or("").trim();
            if text.is_empty() {
                return;
            }

            let target_vec = if is_inside_title {
                title_accumulator
            } else {
                words
            };

            if self.is_non_segmented(text) {
                for c in text.chars().filter(|c| !c.is_whitespace()) {
                    target_vec.push(self.create_word(
                        c.to_string(),
                        is_added,
                        is_red,
                        is_italic,
                        is_inside_title,
                        is_divine,
                        parent_lex,
                        language,
                    ));
                }
            } else {
                for piece in text.split_whitespace() {
                    target_vec.push(self.create_word(
                        piece.to_string(),
                        is_added,
                        is_red,
                        is_italic,
                        is_inside_title,
                        is_divine,
                        parent_lex,
                        language,
                    ));
                }
            }
        } else {
            for child in node.children() {
                self.walk_osis(
                    child,
                    words,
                    verse_notes,
                    title_accumulator,
                    parent_lex,
                    is_red,
                    is_added,
                    is_italic,
                    is_inside_title,
                    is_inside_note,
                    is_divine,
                    language,
                );
            }
        }
    }

}