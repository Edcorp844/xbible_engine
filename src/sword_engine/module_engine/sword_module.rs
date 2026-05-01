#[derive(Debug, Clone)]
#[derive(uniffi::Record)]
pub struct ModuleBook {
    pub name: String,
    pub chapters: Vec<ModuleChapter>,
}

#[derive(Debug, Clone)]
#[derive(uniffi::Record)]
pub struct ModuleChapter {
    pub number: i32,
    pub verse_count: i32,
}

#[derive(Debug, Clone)]
#[derive(uniffi::Record)]
pub struct SwordModule {
    pub name: String,
    pub description: String,
    pub category: String,
    pub language: String,
    pub source: String,
    pub version: String,
    pub delta: String,
    pub cipher_key: String,
    pub features: Vec<String>,
}
