use log::info;
use xbible_engine::engines::xbible_engine::engine::XBibleEngine;

fn main() {
    xbible_engine::init_logging();
    let engine = XBibleEngine::new();

    info!("[Test] Fetching Bible modules...");
    let bible_modules = engine.get_bible_modules();
    info!("[Test] Found {} Bible module(s)", bible_modules.len());

    let Some(first_module) = bible_modules.first() else {
        info!("[Test] No Bible modules installed!");
        return;
    };

    info!("[Test] Using first module: {}", first_module.name);

    info!("[Test] Fetching books for module '{}'...", first_module.name);
    let books = engine.get_books(&first_module.name);
    info!("[Test] Found {} book(s)", books.len());

    let Some(first_book) = books.first() else {
        info!("[Test] Module '{}' has no books available!", first_module.name);
        return;
    };

    info!("[Test] Using first book: {}", first_book.name);

    let first_chapter_num = first_book
        .chapters
        .first()
        .map(|c| c.number)
        .unwrap_or(1);

    let reference = format!("{} {}", first_book.name, first_chapter_num);
    info!("[Test] Querying chapter sections for reference: '{}'...", reference);

    let sections = engine.get_chapter_content(&first_module.name, &reference);
    info!(
        "[Test] Successfully retrieved {} section(s) for '{}'",
        sections.len(),
        reference
    );

    for (idx, section) in sections.iter().enumerate() {
        info!("[Test] Section #{}: {:?}", idx + 1, section);
    }
}