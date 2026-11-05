use xbible_engine::engines::audio_engine::engine::{find_active_verse_leaf, AudioNode};

fn create_mock_navigation_tree() -> AudioNode {
    let verse_1 = AudioNode {
        id: "verse_1".into(),
        title: "Verse 1".into(),
        text: Some("In the beginning...".into()),
        r#type: "Verse".into(),
        children: vec![],
        start_ms: Some(0),
        end_ms: Some(5000),
    };
    let verse_2 = AudioNode {
        id: "verse_2".into(),
        title: "Verse 2".into(),
        text: Some("...God created the heavens and the earth.".into()),
        r#type: "Verse".into(),
        children: vec![],
        start_ms: Some(5001),
        end_ms: Some(10000),
    };

    let chapter_1 = AudioNode {
        id: "chapter_1".into(),
        title: "Chapter 1".into(),
        text: Some(String::new()),
        r#type: "Chapter".into(),
        children: vec![verse_1, verse_2],
        start_ms: Some(0),
        end_ms: Some(10000),
    };

    let section_1 = AudioNode {
        id: "section_1".into(),
        title: "Section 1".into(),
        text: Some(String::new()),
        r#type: "Section".into(),
        children: vec![chapter_1],
        start_ms: None,
        end_ms: None,
    };

    AudioNode {
        id: "root".into(),
        title: "Root Tree".into(),
        text: Some(String::new()),
        r#type: "Root".into(),
        children: vec![section_1],
        start_ms: None,
        end_ms: None,
    }
}

#[test]
fn test_tree_structure_depth() {
    let root = create_mock_navigation_tree();

    assert!(!root.children.is_empty(), "Root should have section children");
    let section = &root.children[0];
    assert_eq!(section.id, "section_1");

    assert!(!section.children.is_empty(), "Section should have chapter children");
    let chapter = &section.children[0];
    assert_eq!(chapter.id, "chapter_1");

    assert!(!chapter.children.is_empty(), "Chapter should have verse children");
}

#[test]
fn test_find_active_verse_leaf_intersections() {
    let root = create_mock_navigation_tree();

    let m1 = find_active_verse_leaf(&root, 2500);
    assert!(m1.is_some(), "should find node at 2500ms");
    assert_eq!(m1.unwrap().id, "verse_1", "expected verse_1 at 2500ms");

    let m2 = find_active_verse_leaf(&root, 7500);
    assert!(m2.is_some(), "should find node at 7500ms");
    assert_eq!(m2.unwrap().id, "verse_2", "expected verse_2 at 7500ms");

    let m_out = find_active_verse_leaf(&root, 99999);
    assert!(m_out.is_none(), "expected None for out-of-bounds timestamp");
}