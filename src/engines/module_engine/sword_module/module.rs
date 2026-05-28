use crate::engines::module_engine::sword_module::module_color::ModuleColor;


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
    pub signature_color: ModuleColor,
}
