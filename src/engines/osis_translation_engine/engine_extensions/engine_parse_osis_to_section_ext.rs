use roxmltree::Document;

use crate::engines::{
    module_engine::module_engine_extensions::module_engine_module_content_ext::{Section, Verse},
    osis_translation_engine::engine::OsisTransilationEngine,
};

impl OsisTransilationEngine {
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

            let (mut words, notes, title_words) = self.parse_osis_content(&language, doc.root());

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
}
