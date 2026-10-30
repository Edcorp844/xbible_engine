use log::info;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use xbible_engine::engines::audio_engine::store::local_store::{
    read_entry_bytes, read_stamped_metadata, read_stub_metadata, scan_installed_modules,
    ModuleLocation, XbaMetadata,
};
use xbible_engine::engines::audio_engine::store::store_api_client::{
    AudioAlbum, AudioCollection, AudioContributor, AudioLanguage, ContributorRole,
    RemoteAudioModuleInfo, RustDownloadProgressHandler, StoreApiClient, StoreApiError,
    StoreDownloadProgressListener, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE,
};

// ─── Progress printer ───

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

// ─── Test harness helpers ───

fn section(title: &str) {
    println!("\n\n═══════════════════════════════════════════════════════════");
    println!("  {}", title);
    println!("═══════════════════════════════════════════════════════════");
}

fn ok<T>(label: &str, value: &T)
where
    T: std::fmt::Debug,
{
    println!("  ✅ {} → {:?}", label, value);
}

fn fail(label: &str, e: StoreApiError) {
    println!("  ❌ {} → {:?}", label, e);
}

macro_rules! try_call {
    ($label:expr, $expr:expr) => {
        match $expr {
            Ok(v) => {
                ok($label, &v);
                Some(v)
            }
            Err(e) => {
                fail($label, e);
                None
            }
        }
    };
}

// ─── Paginated album fetch ───

async fn fetch_all_albums(client: &StoreApiClient) -> Result<Vec<AudioAlbum>, StoreApiError> {
    let mut all: Vec<AudioAlbum> = Vec::new();
    let mut skip: i32 = 0;
    loop {
        let page = client.fetch_albums(MAX_PAGE_SIZE, skip).await?;
        let n = page.len() as i32;
        all.extend(page);
        if n < MAX_PAGE_SIZE {
            break;
        }
        skip += MAX_PAGE_SIZE;
    }
    Ok(all)
}

// ─── Hierarchy walker (module → album → collection → language + narrator) ───

async fn resolve_hierarchy(
    client: &StoreApiClient,
    module: &RemoteAudioModuleInfo,
) -> Result<
    (
        AudioLanguage,
        AudioCollection,
        AudioAlbum,
        Option<AudioContributor>,
    ),
    StoreApiError,
> {
    // ── 1. Find the album ──────────────────────────────────────────────
    let album = match module.album_id.as_ref().map(|r| r.id.clone()) {
        Some(album_id) => {
            println!("  [hierarchy] fast path: album_id={}", album_id);
            client.fetch_album(album_id).await?
        }
        None => {
            println!("  [hierarchy] module has no album_id — scanning all albums");
            let all_albums = fetch_all_albums(client).await?;
            println!("  [hierarchy] {} albums to scan", all_albums.len());

            let mut found: Option<AudioAlbum> = None;
            for (i, a) in all_albums.iter().enumerate() {
                let full = client.fetch_album(a.id.clone()).await?;
                println!(
                    "  [hierarchy]   {:>3}. {:?} ({} modules)",
                    i + 1,
                    a.title,
                    full.modules.len()
                );
                if full.modules.iter().any(|m| m.unique_id == module.unique_id) {
                    println!("  [hierarchy]   ✅ match found in {:?}", a.title);
                    found = Some(full);
                    break;
                }
            }

            found.ok_or_else(|| StoreApiError::NotFound {
                id: format!(
                    "module {} not in any album ({} albums scanned). \
                     In Hygraph, set the module's `album` reference, then re-publish.",
                    module.unique_id,
                    all_albums.len()
                ),
            })?
        }
    };

    // ── 2. Collection ──────────────────────────────────────────────────
    let collection_id = album
        .collection_id
        .as_ref()
        .map(|r| r.id.clone())
        .ok_or_else(|| StoreApiError::NotFound {
            id: format!(
                "album {:?} has no collection. In Hygraph, set the album's `collection` reference.",
                album.title
            ),
        })?;
    let collection = client.fetch_collection(collection_id).await?;

    // ── 3. Language ────────────────────────────────────────────────────
    let language_id = collection
        .language_id
        .as_ref()
        .map(|r| r.id.clone())
        .ok_or_else(|| StoreApiError::NotFound {
            id: format!(
                "collection {:?} has no language. In Hygraph, set the collection's `language` reference.",
                collection.name
            ),
        })?;
    let language = client.fetch_language(language_id).await?;

    // ── 4. Narrator of this collection (first match, if any) ──────────
    let coll_id = collection.id.clone();
    let contributor = client
        .fetch_contributors_by_role(ContributorRole::Narrator, MAX_PAGE_SIZE, 0)
        .await
        .ok()
        .and_then(|list| {
            list.into_iter()
                .find(|c| c.collection_ids().any(|cid| cid == &coll_id))
        });

    Ok((language, collection, album, contributor))
}

// ─── Diagnostic: dump the albums + modules relationship ───

async fn diagnose_albums(client: &StoreApiClient, wanted_unique_id: &str) {
    section("DIAGNOSTIC: albums and their modules");
    println!("  looking for module uniqueId={:?}", wanted_unique_id);

    match fetch_all_albums(client).await {
        Ok(albums) => {
            println!("  found {} albums total", albums.len());
            if albums.is_empty() {
                println!("  ⚠️  0 albums returned — either no entries exist in the Album model,");
                println!("      or the API token doesn't have read access to `albums`.");
                return;
            }
            for a in &albums {
                match client.fetch_album(a.id.clone()).await {
                    Ok(full) => {
                        let hit = full
                            .modules
                            .iter()
                            .any(|m| m.unique_id == wanted_unique_id);
                        println!(
                            "  {} {:?} ({}) → {} modules, collection_id={:?}",
                            if hit { "✅" } else { "  " },
                            a.title,
                            a.id,
                            full.modules.len(),
                            full.collection_id.as_ref().map(|c| c.id.as_str()),
                        );
                        if full.modules.is_empty() {
                            println!(
                                "     ⚠️  this album has NO modules linked. \
                                 In Hygraph: open the Album entry → set `xBibleAudioModules` → save → publish."
                            );
                        }
                    }
                    Err(e) => println!("  ❌ fetch_album({}) → {:?}", a.id, e),
                }
            }
        }
        Err(e) => println!("  ❌ fetch_all_albums → {:?}", e),
    }
}

// ─── Local cache inspection ───
//
// Uses the real `local_store` API. Recursively finds every `*.xba` under the
// cache root, then prints:
//   - the file's size
//   - the stub metadata (unique_id, display_title, version, language, contributor,
//     description, source_url, duration_ms, features, artwork_file)
//   - the organization stamp (language, contributor, collection, album, artwork
//     entries, stamped_at, stamped_by_version)
//   - the actual artwork entries present in the archive (sizes only)
//   - a one-line SUMMARY

fn walk_xba_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_xba_files(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("xba") {
            out.push(path);
        }
    }
}

fn print_stub(stub: &xbible_engine::engines::audio_engine::store::local_store::XbaMetadataStub) {
    println!("  ── stub (original .xba metadata.json) ──");
    println!("    unique_id:      {:?}", stub.unique_id);
    println!("    display_title:  {:?}", stub.display_title);
    println!("    version:        {}", stub.version);
    println!("    language:       {:?}", stub.language);
    println!("    contributor:    {:?}", stub.contributor);
    println!("    duration_ms:    {}", stub.duration_ms);
    println!("    source_url:     {:?}", stub.source_url);
    println!("    features:       {:?}", stub.features);
    println!("    artwork_file:   {:?}", stub.artwork_file);
    if stub.description.is_empty() {
        println!("    description:    <empty>");
    } else {
        let preview: String = stub.description.chars().take(120).collect();
        let ellipsis = if stub.description.chars().count() > 120 { "…" } else { "" };
        println!("    description:    {:?}{}", preview, ellipsis);
    }
}

fn print_organization(org: &xbible_engine::engines::audio_engine::store::local_store::LocalOrganization) {
    println!("  ── organization (stamp injected at download) ──");
    println!("    language_code:  {:?}", org.language_code);
    println!("    language_name:  {:?}", org.language_name);
    println!("    contributor:    {:?}", org.contributor_name);
    println!("    collection_id:  {:?}", org.collection_id);
    println!("    collection:     {:?}", org.collection_name);
    if let Some(desc) = org.collection_description.as_ref() {
        let preview: String = desc.chars().take(80).collect();
        let ellipsis = if desc.chars().count() > 80 { "…" } else { "" };
        println!("    collection_description: {:?}{}", preview, ellipsis);
    }
    println!("    album_id:       {:?}", org.album_id);
    println!("    album:          {:?}", org.album_title);
    println!("    album_manifest_version: {:?}", org.album_manifest_version);
    println!("    album_order:    {}", org.album_order);
    println!("    contributor_artwork_entry: {:?}", org.contributor_artwork_entry);
    println!("    album_artwork_entry:       {:?}", org.album_artwork_entry);
    println!("    stamped_at:     {:?}", org.stamped_at);
    println!("    stamped_by:     {:?}", org.stamped_by_version);
}

fn print_artwork_entries(path: &Path, meta: &XbaMetadata) {
    let mut entries: Vec<(&str, Option<&str>)> = vec![
        ("stub artwork_file", meta.stub.artwork_file.as_deref()),
        ("contributor art",   meta.organization.contributor_artwork_entry.as_deref()),
        ("album art",         meta.organization.album_artwork_entry.as_deref()),
    ];
    entries.retain(|(_, name)| name.is_some());

    if entries.is_empty() {
        println!("  ── artwork entries: none declared ──");
        return;
    }
    println!("  ── artwork entries (actual bytes in the zip) ──");
    for (label, name) in entries {
        let name = name.unwrap();
        match read_entry_bytes(path, name) {
            Ok(bytes) if !bytes.is_empty() => {
                // Peek at magic bytes so we can confirm it's really an image.
                let magic = match bytes.get(..4) {
                    Some([0x89, b'P', b'N', b'G']) => "PNG",
                    Some([0xFF, 0xD8, 0xFF, _])       => "JPEG",
                    Some([b'G', b'I', b'F', b'8'])    => "GIF",
                    Some([b'R', b'I', b'F', b'F'])    => "WEBP/RIFF",
                    _                                  => "unknown",
                };
                println!(
                    "    {} → {:?} ({} bytes, magic: {})",
                    label,
                    name,
                    bytes.len(),
                    magic
                );
            }
            Ok(_) => println!("    {} → {:?} (EMPTY file)", label, name),
            Err(e) => println!("    {} → {:?} (read failed: {:?})", label, name, e),
        }
    }
}

fn inspect_downloaded_modules(root: &Path) {
    section("7. Inspect local module cache");

    let mut files: Vec<PathBuf> = Vec::new();
    walk_xba_files(root, &mut files);
    files.sort();

    if files.is_empty() {
        println!("  ⚠️  No .xba files found under {:?}", root);
        return;
    }

    println!("  found {} .xba file(s) under {:?}\n", files.len(), root);

    for path in &files {
        println!("───────────────────────────────────────────────────────────");
        println!("  FILE: {}", path.display());

        let size_bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        println!(
            "  size: {} bytes ({:.2} KiB)",
            size_bytes,
            size_bytes as f64 / 1024.0
        );

        match read_stamped_metadata(path) {
            Ok(meta) => {
                print_stub(&meta.stub);
                print_organization(&meta.organization);
                print_artwork_entries(path, &meta);

                println!(
                    "  ✅ SUMMARY: title={:?} | collection={:?} | album={:?} | language={:?}({:?}) | narrator={:?} | version={} | stamped_at={:?}",
                    meta.stub.display_title,
                    meta.organization.collection_name,
                    meta.organization.album_title,
                    meta.organization.language_code,
                    meta.organization.language_name,
                    meta.organization.contributor_name,
                    meta.stub.version,
                    meta.organization.stamped_at,
                );
            }
            Err(e) => {
                println!("  ❌ read_stamped_metadata failed: {:?}", e);
                println!("     falling back to read_stub_metadata...");
                match read_stub_metadata(path) {
                    Ok(stub) => {
                        print_stub(&stub);
                        println!(
                            "  ⚠️  UNSTAMPED — this .xba has no organization block. \
                             It was either not produced by `download_and_install_module`, \
                             or the stamp step failed."
                        );
                        println!(
                            "  SUMMARY (stub only): title={:?} | language={:?} | contributor={:?}",
                            stub.display_title, stub.language, stub.contributor,
                        );
                    }
                    Err(e2) => println!("     ❌ stub fallback also failed: {:?}", e2),
                }
            }
        }
    }

    println!("\n───────────────────────────────────────────────────────────");
}

// ─── Cross-check: does scan_installed_modules round-trip cleanly? ───
//
// This is the function the rest of the app actually consumes to build the
// "installed" list. If it doesn't return the same modules we just printed,
// the integration is broken.

fn verify_scan_round_trip(root: &Path) {
    section("8. Verify scan_installed_modules round-trip");

    let scanned: Vec<RemoteAudioModuleInfo> = scan_installed_modules(root);
    println!("  scan_installed_modules → {} module(s)\n", scanned.len());

    if scanned.is_empty() {
        println!("  ⚠️  scan returned nothing. Check stamped_at / stamped_by_version in the .xba.");
        return;
    }

    for m in &scanned {
        println!("───────────────────────────────────────────────────────────");
        println!("  unique_id:        {:?}", m.unique_id);
        println!("  display_title:    {:?}", m.display_title);
        println!("  contributor:      {:?}", m.contributor);
        println!("  duration_ms:      {}", m.duration_ms);
        println!("  version:          {}", m.version);
        println!("  source_url:       {:?}", m.source_url);
        println!("  features:         {:?}", m.features.features);
        println!("  is_installed:     {}", m.is_installed);
        println!("  status:           {:?}", m.status);
        println!("  album_id:         {:?}", m.album_id);
        println!(
            "  artwork_bytes:    {}",
            m.artwork_bytes.as_ref().map(|b| format!("{} bytes ({:?})", b.len(), m.artwork_mime)).unwrap_or("None".into())
        );
        println!(
            "  contributor art:  {}",
            m.contributor_artwork_bytes.as_ref().map(|b| format!("{} bytes ({:?})", b.len(), m.contributor_artwork_mime)).unwrap_or("None".into())
        );
        println!(
            "  album art:        {}",
            m.album_artwork_bytes.as_ref().map(|b| format!("{} bytes ({:?})", b.len(), m.album_artwork_mime)).unwrap_or("None".into())
        );
        println!(
            "  description:      {}",
            m.description.as_deref().map(|d| {
                let p: String = d.chars().take(80).collect();
                if d.chars().count() > 80 { format!("{:?}…", p) } else { format!("{:?}", p) }
            }).unwrap_or("None".into())
        );
    }
}

// ─── Main ───

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    xbible_engine::init_logging();

    let endpoint =
        "https://ap-south-1.cdn.hygraph.com/content/cmpwxdh8104yx07w6h1ffpokb/master".to_string();
    let client = StoreApiClient::new(endpoint, None);

    let target_dir = "./local_modules_cache".to_string();
    std::fs::create_dir_all(&target_dir)?;

    // ═══════════════════════════════════════════════════════════
    // 1. LANGUAGES
    // ═══════════════════════════════════════════════════════════
    section("1. Languages");

    let languages: Option<Vec<AudioLanguage>> = try_call!(
        "fetch_languages(10, 0)",
        client.fetch_languages(10, 0).await
    );

    let _ = try_call!(
        "fetch_languages_with_collections(5, 0)",
        client.fetch_languages_with_collections(5, 0).await
    );

    if let Some(first_lang) = languages.as_ref().and_then(|v| v.first()) {
        let _ = try_call!(
            "fetch_language(first.id)",
            client.fetch_language(first_lang.id.clone()).await
        );
    } else {
        println!("  ⚠️  No languages returned — skipping fetch_language");
    }

    // ═══════════════════════════════════════════════════════════
    // 2. COLLECTIONS
    // ═══════════════════════════════════════════════════════════
    section("2. Collections");

    let collections: Option<Vec<AudioCollection>> = try_call!(
        "fetch_collections(10, 0)",
        client.fetch_collections(10, 0).await
    );

    let _ = try_call!(
        "fetch_collections_with_albums(5, 0)",
        client.fetch_collections_with_albums(5, 0).await
    );

    if let Some(lang_code) = languages
        .as_ref()
        .and_then(|v| v.first())
        .map(|l| l.code.clone())
    {
        let _ = try_call!(
            &format!("fetch_collections_by_language({:?}, 10, 0)", lang_code),
            client
                .fetch_collections_by_language(lang_code.clone(), 10, 0)
                .await
        );
    }

    let first_collection_id = collections
        .as_ref()
        .and_then(|v| v.first())
        .map(|c| c.id.clone());

    if let Some(id) = first_collection_id.as_ref() {
        let _ = try_call!(
            "fetch_collection(first.id) [full tree]",
            client.fetch_collection(id.clone()).await
        );
    }

    // ═══════════════════════════════════════════════════════════
    // 3. ALBUMS
    // ═══════════════════════════════════════════════════════════
    section("3. Albums");

    let albums: Option<Vec<AudioAlbum>> =
        try_call!("fetch_albums(10, 0)", client.fetch_albums(10, 0).await);

    if let Some(cid) = first_collection_id.as_ref() {
        let _ = try_call!(
            "fetch_albums_by_collection(first_collection_id, 10, 0)",
            client.fetch_albums_by_collection(cid.clone(), 10, 0).await
        );
    }

    if let Some(first_album_id) = albums
        .as_ref()
        .and_then(|v| v.first())
        .map(|a| a.id.clone())
    {
        let _ = try_call!(
            "fetch_album(first.id) [with modules]",
            client.fetch_album(first_album_id).await
        );
    }

    // ═══════════════════════════════════════════════════════════
    // 4. CONTRIBUTORS
    // ═══════════════════════════════════════════════════════════
    section("4. Contributors");

    let contributors: Option<Vec<AudioContributor>> = try_call!(
        "fetch_contributors(10, 0)",
        client.fetch_contributors(10, 0).await
    );

    let _ = try_call!(
        "fetch_contributors_by_role(Narrator, 10, 0)",
        client
            .fetch_contributors_by_role(ContributorRole::Narrator, 10, 0)
            .await
    );

    if let Some(cid) = contributors
        .as_ref()
        .and_then(|v| v.first())
        .map(|c| c.id.clone())
    {
        let _ = try_call!(
            "fetch_contributor(first.id)",
            client.fetch_contributor(cid).await
        );
    }

    // ═══════════════════════════════════════════════════════════
    // 5. AUDIO MODULES (paginated)
    // ═══════════════════════════════════════════════════════════
    section("5. Audio Modules (paginated)");

    let page_1: Option<Vec<RemoteAudioModuleInfo>> = try_call!(
        "fetch_audio_modules(DEFAULT_PAGE_SIZE, 0)",
        client.fetch_audio_modules(DEFAULT_PAGE_SIZE, 0).await
    );

    let _ = try_call!(
        "fetch_audio_modules(MAX_PAGE_SIZE, 0)",
        client.fetch_audio_modules(MAX_PAGE_SIZE, 0).await
    );

    let _ = try_call!(
        "fetch_audio_modules(9999, 0) [clamps to MAX]",
        client.fetch_audio_modules(9999, 0).await
    );

    // ═══════════════════════════════════════════════════════════
    // 6. DOWNLOAD TEST
    // ═══════════════════════════════════════════════════════════
    section("6. Download");

    let module_to_download = page_1.as_ref().and_then(|v| v.iter().last()).cloned();

    if let Some(first_module) = module_to_download {
        info!(
            "Downloading package profile: {} ({})",
            first_module.display_title, first_module.unique_id
        );

        let hierarchy = resolve_hierarchy(&client, &first_module).await;
        match hierarchy {
            Ok((language, collection, album, contributor)) => {
                println!(
                    "  ✅ hierarchy: {} / {} / {} / {}",
                    language.code,
                    contributor.as_ref().map(|c| c.name.as_str()).unwrap_or("—"),
                    collection.name,
                    album.title
                );

                let location = ModuleLocation::from_hierarchy(
                    &language,
                    &collection,
                    &album,
                    contributor.as_ref(),
                    &first_module,
                );
                println!(
                    "  → target: {:?}/{}",
                    location.folder_relative(),
                    location.file_name()
                );

                let handler = Arc::new(TerminalProgressPrinter);
                let progress_listener = Arc::new(StoreDownloadProgressListener::new_native(
                    Some(first_module.unique_id.clone()),
                    handler,
                ));

                match client
                    .download_and_install_module(
                        first_module.clone(),
                        target_dir.clone(),
                        location.clone(),
                        collection,
                        album,
                        contributor,
                        progress_listener,
                    )
                    .await
                {
                    Ok(path) => {
                        println!("\n  ✅ Download complete → {}", path);

                        let expected = std::path::Path::new(&target_dir)
                            .join(location.folder_relative())
                            .join(location.file_name());
                        println!(
                            "  {} expected path exists: {:?}",
                            if expected.exists() { "✅" } else { "❌" },
                            expected
                        );
                    }
                    Err(e) => {
                        println!("\n  ❌ Download failed → {:?}", e);
                    }
                }
            }
            Err(e) => {
                println!("  ❌ hierarchy resolution failed → {:?}", e);
                println!("  ⚠️  Skipping download. Running diagnostic...");
                diagnose_albums(&client, &first_module.unique_id).await;
            }
        }
    } else {
        println!("  ⚠️  No modules available — skipping download");
    }

    // ═══════════════════════════════════════════════════════════
    // 7. INSPECT LOCAL CACHE
    // ═══════════════════════════════════════════════════════════
    inspect_downloaded_modules(Path::new(&target_dir));

    // ═══════════════════════════════════════════════════════════
    // 8. VERIFY scan_installed_modules ROUND-TRIP
    // ═══════════════════════════════════════════════════════════
    verify_scan_round_trip(Path::new(&target_dir));

    section("Done");
    Ok(())
}