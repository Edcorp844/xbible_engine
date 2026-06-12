use std::sync::Arc;
use xbible_engine::engines::audio_engine::store::store_api_client::{
    RustDownloadProgressHandler, StoreApiClient, StoreDownloadProgressListener,
};

// Define a local tracker block for output
struct TerminalProgressPrinter;

impl RustDownloadProgressHandler for TerminalProgressPrinter {
    fn on_progress(&self, unique_id: String, bytes_written: u64, total_bytes: Option<u64>) {
        if let Some(total) = total_bytes {
            let percentage = (bytes_written as f64 / total as f64) * 100.0;
            print!(
                "\r[{}] Downloading: {:.2}% ({}/{} bytes)",
                unique_id, percentage, bytes_written, total
            );
        } else {
            print!(
                "\r[{}] Streaming chunks: {} bytes received",
                unique_id, bytes_written
            );
        }
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint =
        "https://ap-south-1.cdn.hygraph.com/content/cmpwxdh8104yx07w6h1ffpokb/master".to_string();
    let client = StoreApiClient::new(endpoint, None);

    let target_dir = "./local_modules_cache".to_string();
    std::fs::create_dir_all(&target_dir)?;

    let available_modules = client.fetch_audio_modules().await?;

    if let Some(first_module) = available_modules.first() {
        println!(
            "Downloading package profile: {}",
            first_module.display_title
        );

        // Use our custom non-FFI constructor to bind the printing handler loop natively
        let handler = Arc::new(TerminalProgressPrinter);
        let progress_listener = Arc::new(StoreDownloadProgressListener::new_native(
            Some(first_module.unique_id.clone()),
            handler,
        ));

        let saved_path_string = client
            .download_and_install_module(first_module.clone(), target_dir, progress_listener)
            .await?;

        println!("\n\nArchive box cleanly targeted at: {}", saved_path_string);
    }
    Ok(())
}
