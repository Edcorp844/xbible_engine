use fnv::FnvHasher;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, uniffi::Record)]
pub struct ModuleColor {
    pub hue: f64,
    pub saturation: f64,
    pub brightness: f64,
}

impl ModuleColor {
    pub fn generate(input: &str) -> Self {
        let mut hasher = FnvHasher::default();
        input.hash(&mut hasher);
        let hash = hasher.finish();

        let hue = (hash % 1000) as f64 / 1000.0;

        Self {
            hue,
            saturation: 0.5,
            brightness: 0.45,
        }
    }
}
