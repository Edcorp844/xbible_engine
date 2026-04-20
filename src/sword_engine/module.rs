/// A Sword module metadata structure.
///
/// This represents information about a Sword module (e.g., a Bible translation).
#[derive(Debug, Clone)]
#[derive(uniffi::Record)]
pub struct SwordModule {
    pub name: String,
    pub description: String,
    pub category: String,
    pub language: String,
    pub version: String,
    pub delta: String,
    pub cipher_key: String,
    pub features: Vec<String>,
}

/// A book in a Bible module.
#[derive(Debug, Clone)]
#[derive(uniffi::Record)]
pub struct ModuleBook {
    pub name: String,
    pub chapters: Vec<ModuleChapter>,
}

/// A chapter in a Bible book.
#[derive(Debug, Clone)]
#[derive(uniffi::Record)]
pub struct ModuleChapter {
    pub number: i32,
    pub verse_count: i32,
}
