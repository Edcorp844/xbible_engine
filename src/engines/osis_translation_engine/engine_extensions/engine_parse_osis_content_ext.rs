use roxmltree::Node;

use crate::engines::{
    module_engine::module_engine_extensions::module_engine_module_content_ext::{LexicalInfo, Word},
    osis_translation_engine::engine::OsisTransilationEngine,
};

impl OsisTransilationEngine {
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

        // Compute contiguous group and added-word boundaries
        self.mark_group_boundaries(&mut words);
        if !title_words.is_empty() {
            self.mark_group_boundaries(&mut title_words);
        }

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

            // 1. Tag Parsing & State Extraction
            if node.has_tag_name("title") {
                traversing_title = true;
            } else if node.has_tag_name("w") {
                let raw_lemma = node.attribute("lemma").unwrap_or("");
                let raw_morph = node.attribute("morph").unwrap_or("");
                let raw_gloss = node.attribute("gloss").map(|g| g.to_string());

                // Parse Strong's numbers
                let strongs: Vec<String> = raw_lemma
                    .split_whitespace()
                    .filter(|s| s.starts_with("strong:"))
                    .map(|s| s.trim_start_matches("strong:").to_string())
                    .collect();

                // Extract clean textual lemma if present
                let clean_lemma = if !raw_lemma.is_empty() {
                    let non_strong: Vec<&str> = raw_lemma
                        .split_whitespace()
                        .filter(|s| !s.starts_with("strong:"))
                        .collect();
                    if !non_strong.is_empty() {
                        Some(non_strong.join(" "))
                    } else {
                        None
                    }
                } else {
                    None
                };

                // Parse morphology tags into Vec<String>
                let morph: Vec<String> = raw_morph
                    .split_whitespace()
                    .map(|m| {
                        m.trim_start_matches("robinson:")
                            .trim_start_matches("packard:")
                            .trim_start_matches("strongMorph:")
                            .to_string()
                    })
                    .filter(|m| !m.is_empty())
                    .collect();

                if !strongs.is_empty()
                    || clean_lemma.is_some()
                    || raw_gloss.is_some()
                    || !morph.is_empty()
                {
                    active_lex_owned = Some(LexicalInfo {
                        strongs,
                        lemma: clean_lemma,
                        gloss: raw_gloss,
                        morph,
                    });
                }
            } else if node.has_tag_name("divineName") {
                active_divine = true;
            } else if node.has_tag_name("q") {
                if node.attribute("who") == Some("Jesus")
                    || node.attribute("marker") == Some("red")
                {
                    active_red = true;
                }
            } else if node.has_tag_name("transChange") {
                if node.attribute("type") == Some("added") || node.attribute("type").is_none() {
                    active_added = true;
                }
            } else if node.has_tag_name("hi") {
                let hi_type = node.attribute("type").unwrap_or("");
                if hi_type == "italic" || hi_type == "oblique" {
                    active_italic = true;
                }
            } else if node.has_tag_name("note") {
                let note_type = node.attribute("type").unwrap_or("explanation");
                let note_text = self.collect_note_text(node);

                if !note_text.is_empty() {
                    let formatted_note = if note_type != "explanation" {
                        format!("[{}] {}", note_type, note_text)
                    } else {
                        note_text
                    };

                    let target_vec = if traversing_title {
                        &mut *title_accumulator
                    } else {
                        &mut *words
                    };

                    // Check if note lives inside <w> or directly follows parsed words
                    let is_inside_w = node.ancestors().any(|a| a.has_tag_name("w"));
                    if is_inside_w || !target_vec.is_empty() {
                        if let Some(last_word) = target_vec.last_mut() {
                            match &mut last_word.note {
                                Some(existing) => {
                                    existing.push_str("\n");
                                    existing.push_str(&formatted_note);
                                }
                                None => {
                                    last_word.note = Some(formatted_note);
                                }
                            }
                            return;
                        }
                    }

                    // Fallback to verse-level note if standalone
                    verse_notes.push(formatted_note);
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
                    is_inside_note,
                    active_divine,
                    language,
                );
            }
        } else if node.is_text() {
            if is_inside_note {
                return;
            }

            let raw_text = node.text().unwrap_or("");
            if raw_text.trim().is_empty() {
                return;
            }

            let target_vec = if is_inside_title {
                title_accumulator
            } else {
                words
            };

            if self.is_non_segmented(raw_text) {
                for c in raw_text.chars().filter(|c| !c.is_whitespace()) {
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
                for piece in raw_text.split_whitespace() {
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

 

    fn mark_group_boundaries(&self, words: &mut [Word]) {
        let len = words.len();
        if len == 0 {
            return;
        }

        for i in 0..len {
            if words[i].is_added {
                words[i].is_first_added = i == 0 || !words[i - 1].is_added;
                words[i].is_last_added = i == len - 1 || !words[i + 1].is_added;
            } else {
                words[i].is_first_added = false;
                words[i].is_last_added = false;
            }

            let prev_same = i > 0
                && words[i - 1].is_red == words[i].is_red
                && words[i - 1].is_italic == words[i].is_italic
                && words[i - 1].is_added == words[i].is_added
                && words[i - 1].is_title == words[i].is_title;

            let next_same = i < len - 1
                && words[i + 1].is_red == words[i].is_red
                && words[i + 1].is_italic == words[i].is_italic
                && words[i + 1].is_added == words[i].is_added
                && words[i + 1].is_title == words[i].is_title;

            words[i].is_first_in_group = !prev_same;
            words[i].is_last_in_group = !next_same;
        }
    }
}