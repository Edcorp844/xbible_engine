use roxmltree::{Document, Node};

use crate::sword_engine::module_engine::sword_engine_module_content_ext::{LexicalInfo, Section, TextDirection, Verse, Word};

pub struct OsisTransilationEngine {}

impl OsisTransilationEngine {
    pub fn new() -> Self {
        Self {}
    }

    /// The "Master" Fix: Groups a list of verses into the minimum number of Sections.
    /// If Verse 2, 3, and 4 have no <title> tags, they are added to Verse 1's Section.
    pub fn parse_osis_list_to_sections(
        &self,
        language: String,
        fragments: Vec<(String, String)>,
    ) -> Vec<Section> {
        let mut sections: Vec<Section> = Vec::new();

        for (key, osis) in fragments {
            let wrapped_osis = format!("<root>{}</root>", osis);
            let doc = match Document::parse(&wrapped_osis) {
                Ok(d) => d,
                Err(_) => continue,
            };

            let (mut words, notes, title_words) =
                self.parse_osis_content(&language, doc.root());

            if words.is_empty() && title_words.is_none() {
                continue;
            }

            self.apply_group_metadata(&mut words);

            let verse = Verse {
                number: self.extract_verse_number(&key),
                osis_id: key.clone(),
                words,
                notes,
                is_paragraph_start: osis.contains("type=\"paragraph\"") || key.ends_with(":1"),
            };

            // --- THE CORE FIX ---
            // If we have a title, we MUST start a new section.
            if let Some(mut t_words) = title_words {
                self.apply_group_metadata(&mut t_words);

                let text_direction = self.detect_direction(&verse);
                sections.push(Section {
                    title: t_words,
                    verses: vec![verse],
                    text_direction,
                });
            } else {
                // If there is NO title, try to append to the existing last section.
                if let Some(last_section) = sections.last_mut() {
                    last_section.verses.push(verse);
                } else {
                    // No sections exist yet (e.g. Verse 1 has no title), create the first one.
                    let text_direction = self.detect_direction(&verse);
                    sections.push(Section {
                        title: Vec::new(),
                        verses: vec![verse],
                        text_direction,
                    });
                }
            }
        }
        sections
    }

    /// Single-verse entry point now forced to return a single section
    pub fn parse_osis_to_sections(
        &self,
        language: String,
        osis: &str,
        verse_key: Option<String>,
    ) -> Vec<Section> {
        let key = verse_key.unwrap_or_default();
        self.parse_osis_list_to_sections(language, vec![(key, osis.to_string())])
    }

    fn parse_osis_content(
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

    fn create_word(
        &self,
        text: String,
        _is_added: bool,
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
            lex: lex.cloned(),
            note: None,
            is_first_in_group: false,
            is_last_in_group: false,
            is_title: is_inside_title,
            is_punctuation: is_punct,
            language: language.to_string(),
        }
    }

    fn is_non_segmented(&self, text: &str) -> bool {
        text.chars().any(|c| {
            ('\u{4E00}'..='\u{9FFF}').contains(&c) || ('\u{3040}'..='\u{30FF}').contains(&c)
        })
    }

    fn collect_note_text(&self, node: Node) -> String {
        node.descendants()
            .filter_map(|n| n.text())
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string()
    }

    fn apply_group_metadata(&self, words: &mut [Word]) {
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

    fn extract_verse_number(&self, key: &str) -> i32 {
        key.split(|c| c == '.' || c == ':')
            .last()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    fn detect_direction(&self, verse: &Verse) -> TextDirection {
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
