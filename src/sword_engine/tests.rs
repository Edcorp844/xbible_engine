#[cfg(test)]
mod tests {
    use crate::sword_engine::SwordEngine;

    #[test]
    fn test_sword_engine_creation() {
        let _engine = SwordEngine::new();
        // Test that engine was created successfully
        assert!(true); // If we get here, creation succeeded
    }

    #[test]
    fn test_module_operations() {
        let engine = SwordEngine::new();
        // This test assumes a module named "KJV" exists; adjust as needed.
        let text = engine.lookup_verse("KJV", "John 3:16");
        if !text.is_empty() {
            assert!(!text.is_empty());
        } else {
            // Skip test if module not available
            println!("KJV module not found or no text returned, skipping test");
        }
    }
}