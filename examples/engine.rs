use xbible_engine::{bible_api::BibleEngine, sword_engine::module_engine::sword_engine_lexicon_ext::LexiconQuery};

fn main() {
    let engine = BibleEngine::new();

    let greek_query = "G3056"; // Example Strong's number for "agape" (love) in Greek
    let language = "Greek";

    let hebrew_query = "H2617"; // Example Strong's number for "hesed" (loving-kindness) in Hebrew
    let language_hebrew = "Hebrew";
    
    let hebrew_query2 = "H1"; // Testing with a common Hebrew number
    let language_hebrew2 = "Hebrew";

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

    println!("Lexicon results for {}: {:?}", greek_query, response);

    let hebrew_response = engine.lookup_strongs_number(hebrew_lexicon_query);

    println!("Lexicon results for {}: ", hebrew_query);
    for result in &hebrew_response.results {
        println!("  - Module: {}, Key: {}", result.module_name, result.key);
    }
    if hebrew_response.results.is_empty() {
        println!("  (No results found)");
    }
    
    // Test with H1
    let hebrew_lexicon_query2 = LexiconQuery {
        strongs_number: hebrew_query2.to_string(),
        language: language_hebrew2.to_string(),
    };
    let hebrew_response2 = engine.lookup_strongs_number(hebrew_lexicon_query2);

    println!("Lexicon results for {}: ", hebrew_query2);
    for result in &hebrew_response2.results {
        println!("  - Module: {}, Key: {}", result.module_name, result.key);
    }
    if hebrew_response2.results.is_empty() {
        println!("  (No results found)");
    }
}
