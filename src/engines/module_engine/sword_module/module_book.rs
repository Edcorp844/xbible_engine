use crate::engines::module_engine::sword_module::module_chapter::ModuleChapter;


#[derive(Debug, Clone)]
#[derive(uniffi::Record)]
pub struct ModuleBook {
    pub name: String,
    pub chapters: Vec<ModuleChapter>,
}