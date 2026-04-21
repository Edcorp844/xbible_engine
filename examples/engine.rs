use xbible_engine::sword_engine::SwordEngine;

fn main() {
    let engine = SwordEngine::new();

    println!("Listing remote Sword sources...");
    let sources = engine.get_remote_source_list();

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

    println!("\nInstalled local modules:");
    let installed = engine.get_modules();
    println!("{} installed modules found", installed.len());
}
