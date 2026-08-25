use log::{error, info};
use xbible_engine::engines::{
    module_engine::module_engine_extensions::module_engine_search_ext::SearchType,
    xbible_engine::engine::XBibleEngine,
};

fn main() {
    xbible_engine::init_logging();
    let engine = XBibleEngine::new();

    let modules = engine.get_available_modules();
    info!("Available modules count: {}", modules.len());

    if let Some(module) = modules.first() {
        info!("Targeting module: {}", module.name);
        let search_word = "God".to_string();

        let results = engine.search(
            module.name.clone(),
            search_word.clone(),
            SearchType::RegularExpression,
        );
        info!("Search results count: {}", results.hits.len());
        info!("{:?}", results);
    } else {
        error!("No Available modules found to execute search");
    }
}
