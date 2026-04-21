use crate::sword_engine::module_engine::sword_engine::SwordEngine;


#[derive(Debug, Clone, PartialEq)]
pub enum Testament {
    Old,
    New,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CategorizedBook {
    pub name: String,
    pub testament: Testament,
    pub index: usize,
}

impl SwordEngine {
    /// Returns books grouped by Testament based on the standard 66-book canon.
    /// This uses your existing bible_structure internal logic.
    pub fn get_categorized_books(&self, module_name: &str) -> Vec<CategorizedBook> {
        // Accessing your existing structure vector
        let raw_books = self.get_bible_structure(module_name);

        raw_books
            .into_iter()
            .enumerate()
            .map(|(i, book)| {
                // The Protestant Canon split: 0-38 (39 books) is Old Testament.
                // 39 and above (Matthew onwards) is New Testament.
                let testament = if i < 39 {
                    Testament::Old
                } else {
                    Testament::New
                };

                CategorizedBook {
                    name: book.name,
                    testament,
                    index: i,
                }
            })
            .collect()
    }

    /// Helper to safely get a book name by index without the UI needing to bounds-check.
    pub fn get_book_name(&self, module_name: &str, index: usize) -> String {
        self.get_bible_structure(module_name)
            .get(index)
            .map(|b| b.name.clone())
            .unwrap_or_else(|| "Unknown Book".to_string())
    }

    /// Returns the number of chapters for a specific book in a module.
    /// Useful for building the next grid (the Chapter selector).
    pub fn get_chapter_count(&self, module_name: &str, book_index: usize) -> i32 {
        self.get_bible_structure(module_name)
            .get(book_index)
            .map(|b| b.chapters.len() as i32)
            .unwrap_or(0)
    }
}
