use xbible_engine::{bible_api::BibleEngine};

fn main() {
    let engine = BibleEngine::new();

    println!("Listing remote Sword sources...");
    let sources = engine.get_remote_sources();

    if sources.is_empty() {
        println!("No remote sources were found.");
        println!("Make sure Sword can access the internet and that the install manager is initialized correctly.");
        return;
    }

    println!("Remote sources (official Sword repositories):");
    for source in &sources {
        println!(" - {}", source);
    }

    let source_name = &sources[0];
    println!("\nFetching modules from source: {}", source_name);
    let modules = engine.fetch_remote_modules(source_name);

    if modules.is_empty() {
        println!("No modules were retrieved from source {}.", source_name);
        return;
    }

    println!("Found {} remote modules from {}:", modules.len(), source_name);
    for module in modules.iter().take(50) {
        println!("- {} [{}] {} ({})", module.name, module.category, module.language, module.version);
    }
    // Try to install a small module for testing
    if let Some(small_module) = modules.iter().find(|m| m.category.contains("Commentaries") || m.category.contains("Dict")) {
        println!("\nAttempting to install small module: {} ({})", small_module.name, small_module.category);
        let install_result = engine.install_module(source_name, &small_module.name);
        println!("Installation result: {}", install_result);

        // Check progress
        let progress = engine.get_download_progress();
        println!("Download progress: {:.1}%", progress * 100.0);
    }
    println!("\nInstalled local modules:");
    let installed = engine.get_available_modules();
    println!("{} Bible modules found", installed.len());
    
    // Show different types of modules separately
    let commentaries = engine.get_commentary_modules();
    println!("{} Commentary modules found", commentaries.len());
    
    let dictionaries = engine.get_dictionary_modules();
    println!("{} Dictionary modules found", dictionaries.len());
    
    let glossaries = engine.get_glossary_modules();
    println!("{} Glossary modules found", glossaries.len());
    
    let lexicons = engine.get_lexicon_modules();
    println!("{} Lexicon modules found", lexicons.len());
    
    let devotionals = engine.get_daily_devotional_modules();
    println!("{} Daily devotional modules found", devotionals.len());
    
    let books = engine.get_book_modules();
    println!("{} Book modules found", books.len());
    
    // Also show all modules including commentaries
    let all_modules = engine.refresh_installed_modules();
    println!("\nTotal modules available: {}", all_modules.len());
    for module in &all_modules {
        println!("- {} [{}] {} ({})", module.name, module.category, module.language, module.version);
    }
}
