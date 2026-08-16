use xbible_engine::engines::{module_engine::module_engine_extensions::module_engine_lexicon_ext::LexiconQuery, xbible_engine::engine::XBibleEngine};

fn main() {
    xbible_engine::init_logging();
    let engine = XBibleEngine::new();

    let greek_query = "G3056"; // Example Strong's number for "agape" (love) in Greek
    let language = "Greek";

    let hebrew_query = "H2617"; // Example Strong's number for "hesed" (loving-kindness) in Hebrew
    let language_hebrew = "Hebrew";

    let dict_modules = engine.get_dictionary_modules();
    println!("Available dictionary modules:");
    for module in &dict_modules {
        println!(" - {} (Language: {})", module.name, module.language);
    }
    
    let lexicon_query = LexiconQuery {
        strongs_number: greek_query.to_string(),  
        language: language.to_string(),
    };

    let hebrew_lexicon_query = LexiconQuery {
        strongs_number: hebrew_query.to_string(),
        language: language_hebrew.to_string(),
    };

    let response = engine.lookup_strongs_number(lexicon_query);

    println!("Lexicon results for {}: {} total results", greek_query, response.results.len());
    for result in &response.results {
        println!("  - Module: {}, Key: {}", result.module_name, result.key);
    }
    
    // Also test H5501 which we know works
    let hebrew_test = LexiconQuery {
        strongs_number: "H5501".to_string(),
        language: "Hebrew".to_string(),
    };
    let hebrew_resp = engine.lookup_strongs_number(hebrew_test);
    println!("\nLexicon results for H5501 (Hebrew): {} total results", hebrew_resp.results.len());
    for result in &hebrew_resp.results {
        println!("  - Module: {}, Key: {}", result.module_name, result.key);
    }
    
    // Test without language
    let hebrew_test2 = LexiconQuery {
        strongs_number: "H5501".to_string(),
        language: "".to_string(),
    };
    let hebrew_resp2 = engine.lookup_strongs_number(hebrew_test2);
    println!("\nLexicon results for H5501 (no language): {} total results", hebrew_resp2.results.len());
    for result in &hebrew_resp2.results {
        println!("  - Module: {}, Key: {}", result.module_name, result.key);
    }

    let hebrew_response = engine.lookup_strongs_number(hebrew_lexicon_query);

    println!("Lexicon results for {}: {:?}", hebrew_query, hebrew_response);
}
