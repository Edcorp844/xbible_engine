use isolang::Language;

use crate::sword_engine::module_engine::sword_engine::SwordEngine;


impl SwordEngine {
    pub fn from_code(&self, code: &str) -> String {
        Language::from_639_1(code)
            .or_else(|| Language::from_639_3(code))
            .map(|l| l.to_name().to_string())
            .unwrap_or_else(|| {
                if code.is_empty() {
                    "Unknown".to_string()
                } else {
                    code.to_uppercase()
                }
            })
    }
}
