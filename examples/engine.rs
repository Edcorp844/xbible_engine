use xbible_engine::sword_engine::SwordEngine;


fn main(){
    let engine = SwordEngine::new();

    // Example: Lookup a verse from the KJV Bible
    let verse_text = engine.lookup_verse("KJV", "John 3:16");
    if !verse_text.is_empty() {
        println!("John 3:16 (KJV): {}", verse_text);
    } else {
        println!("KJV module not found or verse not available");
    }

    // Example: Get list of available modules
    let modules = engine.get_modules();
    println!("Found {} modules", modules.len());

    // Example: Get Bible modules specifically
    let bible_modules = engine.get_bible_modules();
    println!("Found {} Bible modules", bible_modules.len());

}