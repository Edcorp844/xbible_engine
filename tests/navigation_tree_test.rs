#[cfg(test)]
mod tests {
    use xbible_engine::engines::audio_engine::engine::{AudioNode, find_active_verse_leaf};
    
    // Utility helper to build a dummy tree structure matching your exact production schema
    fn create_mock_navigation_tree() -> AudioNode {
        // 1. Create Verses (Leaves) with explicit millisecond brackets
        let verse_1 = AudioNode {
            id: "verse_1".to_string(),
            title: "Verse 1".to_string(),
            text: Some("In the beginning...".to_string()),
            r#type: "Verse".to_string(), // Adjust this if your type is an Enum instead of a String
            children: vec![],
            start_ms: Some(0),
            end_ms: Some(5000),
        };
        let verse_2 = AudioNode {
            id: "verse_2".to_string(),
            title: "Verse 2".to_string(),
            text: Some("...God created the heavens and the earth.".to_string()),
            r#type: "Verse".to_string(),
            children: vec![],
            start_ms: Some(5001),
            end_ms: Some(10000),
        };

        // 2. Create Chapters (Parents of Verses)
        let chapter_1 = AudioNode {
            id: "chapter_1".to_string(),
            title: "Chapter 1".to_string(),
            text: Some("".to_string()),
            r#type: "Chapter".to_string(),
            children: vec![verse_1, verse_2],
            start_ms: Some(0),
            end_ms: Some(10000),
        };

        // 3. Create Sections (Parents of Chapters - What SwiftUI loops through)
        let section_1 = AudioNode {
            id: "section_1".to_string(),
            title: "Section 1".to_string(),
            text: Some("".to_string()),
            r#type: "Section".to_string(),
            children: vec![chapter_1],
            start_ms: None, 
            end_ms: None,
        };

        // 4. Create Root Node
        AudioNode {
            id: "root".to_string(),
            title: "Root Tree".to_string(),
            text: Some("".to_string()),
            r#type: "Root".to_string(),
            children: vec![section_1],
            start_ms: None,
            end_ms: None,
        }
    }

    #[test]
    fn test_tree_structure_depth() {
        let root = create_mock_navigation_tree();
        
        // Assert that Root has a Section layer
        assert!(!root.children.is_empty(), "Root should have section children");
        let section = &root.children[0];
        assert_eq!(section.id, "section_1");

        // Assert that Section has a Chapter layer
        assert!(!section.children.is_empty(), "Section should have chapter children");
        let chapter = &section.children[0];
        assert_eq!(chapter.id, "chapter_1");
        
        // Assert that Chapter has a Verse leaf layer
        assert!(!chapter.children.is_empty(), "Chapter should have verse children");
    }

    #[test]
    fn test_find_active_verse_leaf_intersections() {
        let root = create_mock_navigation_tree();

        // Test time = 2500ms (Should land squarely on Verse 1)
        let match_1 = find_active_verse_leaf(&root, 2500);
        assert!(match_1.is_some(), "Should find a node at 2500ms");
        assert_eq!(match_1.unwrap().id, "verse_1", "Expected verse_1 at 2500ms");

        // Test time = 7500ms (Should land squarely on Verse 2)
        let match_2 = find_active_verse_leaf(&root, 7500);
        assert!(match_2.is_some(), "Should find a node at 7500ms");
        assert_eq!(match_2.unwrap().id, "verse_2", "Expected verse_2 at 7500ms");

        // Test out-of-bounds time = 99999ms (Should return None)
        let match_out_of_bounds = find_active_verse_leaf(&root, 99999);
        assert!(match_out_of_bounds.is_none(), "Should return None for timestamps out of bounds");
    }
}