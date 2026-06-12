use std::{path::Path, sync::Arc};
use xbible_engine::engines::audio_engine::store::store_api_client::{
    StoreApiClient, StoreDownloadProgressListener,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = "https://ap-south-1.cdn.hygraph.com/content/cmpwxdh8104yx07w6h1ffpokb/master".to_string();
    let client = StoreApiClient::new(endpoint, None);

    let target_dir = "./local_modules_cache".to_string();
    std::fs::create_dir_all(&target_dir)?;

    println!("Fetching remote list tracking entries asynchronously...");
    
    let available_modules = client.fetch_audio_modules().await?;

    if let Some(first_module) = available_modules.first() {
        println!("\nDownloading package profile: {}", first_module.display_title);
        
        // ─── FIX: PASS THE EXPECTED OPTION FILTER INSTEAD OF A CLOSURE ───
        // We pass None because we don't need a specific module unique ID filter constraint
        let progress_listener = Arc::new(StoreDownloadProgressListener::new(None));

        // Pass the structural proxy container down to the client stream pump
        let saved_path_string = client.download_and_install_module(
            first_module.clone(), 
            target_dir,
            progress_listener
        ).await?;
        
        println!("\n\nArchive box cleanly targeted and flushed out at: {}", saved_path_string);
        assert!(Path::new(&saved_path_string).exists());
    } else {
        println!("No audio packages returned in the catalog response array.");
    }

    Ok(())
}