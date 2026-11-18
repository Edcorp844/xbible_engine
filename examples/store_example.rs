use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use xbible_engine::XBibleEngine;
use xbible_engine::features::store::store::StoreItem;
use xbible_engine::features::store::store_api::{StoreApi, StoreTaskState};

fn remove_sqlite_file_and_sidecars(db_path: &Path) {
    let base: String = db_path.to_string_lossy().into_owned();
    for candidate in [
        PathBuf::from(&base),
        PathBuf::from(format!("{base}-wal")),
        PathBuf::from(format!("{base}-shm")),
        PathBuf::from(format!("{base}-journal")),
    ] {
        if candidate.exists() {
            if let Err(err) = std::fs::remove_file(&candidate) {
                eprintln!("Warning: could not remove {}: {err}", candidate.display());
            }
        }
    }
}

fn fresh_store_db_path() -> PathBuf {
    let dir: PathBuf = std::env::temp_dir().join("xbible_store_test");
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!(
            "Warning: could not create temp dir {}: {err}",
            dir.display()
        );
    }
    let db_path: PathBuf = dir.join("store_testtmt.db");
    remove_sqlite_file_and_sidecars(&db_path);
    db_path
}

fn poll_until_done(store: &Arc<StoreApi>, task_id: String) -> Result<(), String> {
    loop {
        let status = store.poll_task(task_id.clone());
        match status {
            Some(s) => match s.state {
                StoreTaskState::Queued | StoreTaskState::Running => {
                    println!(
                        "  [{:.0}%] {}",
                        s.progress.fraction * 100.0,
                        s.progress.message
                    );
                }
                StoreTaskState::Completed => {
                    println!("  Completed: {}", s.progress.message);
                    return Ok(());
                }
                StoreTaskState::Failed { error } => {
                    return Err(format!("Task '{task_id}' failed: {error}"));
                }
                StoreTaskState::Cancelled => {
                    return Err(format!("Task '{task_id}' cancelled"));
                }
            },
            None => return Err(format!("Task '{task_id}' disappeared")),
        }

        thread::sleep(Duration::from_millis(250));
    }
}

fn pick_non_bsb_translation(
    store: &Arc<StoreApi>,
    explicit_id: Option<&str>,
) -> Result<StoreItem, String> {
    if let Some(requested) = explicit_id {
        if requested.eq_ignore_ascii_case("BSB") {
            return Err(
                "Requested translation is BSB; expected a translation other than BSB".to_string(),
            );
        }
        if let Some(found) =
            store
                .search(requested.to_string())
                .into_iter()
                .find(|item: &StoreItem| {
                    item.kind == "translation" && item.id.eq_ignore_ascii_case(requested)
                })
        {
            return Ok(found);
        }
    }

    for candidate_id in ["WEB", "ASV", "KJV", "YLT", "DBY"] {
        if let Some(found) =
            store
                .search(candidate_id.to_string())
                .into_iter()
                .find(|item: &StoreItem| {
                    item.kind == "translation" && !item.id.eq_ignore_ascii_case("BSB")
                })
        {
            return Ok(found);
        }
    }

    let shelf_items: Vec<StoreItem> = store.get_shelf(
        "Public Domain Classics".to_string(),
        Some("eng".to_string()),
        0,
        50,
    );
    if let Some(found) = shelf_items
        .into_iter()
        .find(|item: &StoreItem| item.kind == "translation" && !item.id.eq_ignore_ascii_case("BSB"))
    {
        return Ok(found);
    }

    store
        .search("".to_string())
        .into_iter()
        .find(|item: &StoreItem| item.kind == "translation" && !item.id.eq_ignore_ascii_case("BSB"))
        .ok_or_else(|| "no non-BSB translation found in store catalog".to_string())
}

fn pick_commentary(store: &Arc<StoreApi>) -> Option<StoreItem> {
    store
        .search("adam-clarke".to_string())
        .into_iter()
        .find(|item: &StoreItem| item.kind == "commentary")
        .or_else(|| {
            store
                .search("commentary".to_string())
                .into_iter()
                .find(|item: &StoreItem| item.kind == "commentary")
        })
        .or_else(|| {
            store
                .search("".to_string())
                .into_iter()
                .find(|item: &StoreItem| item.kind == "commentary")
        })
}

fn pick_people_entity_set(store: &Arc<StoreApi>) -> Option<StoreItem> {
    store.item("theographic:person".to_string(), "entity_set".to_string())
}

pub fn install_three_items(
    engine: &Arc<XBibleEngine>,
    explicit_bible_id: Option<&str>,
) -> Result<(), String> {
    let store: Arc<StoreApi> = engine.store();

    // 1. Refresh catalog index across registered sources and cache in DB.
    store
        .refresh()
        .map_err(|e| format!("refresh failed: {e:?}"))?;

    // 2. Install a Bible translation that is NOT BSB.
    let bible: StoreItem = pick_non_bsb_translation(&store, explicit_bible_id)?;
    println!("[1/3] Bible translation: '{}' ({})", bible.name, bible.id);
    if bible.installed {
        println!("  '{}' is already installed", bible.id);
    } else {
        let task_id: String = store
            .install_translation_from_source(bible.source_id.clone(), bible.id.clone())
            .map_err(|e| format!("bible install start failed: {e:?}"))?;
        poll_until_done(&store, task_id)?;
    }

    // 3. Locate commentary item (e.g. adam-clarke).
    match pick_commentary(&store) {
        Some(commentary) => {
            println!(
                "[2/3] Commentary: '{}' ({})",
                commentary.name, commentary.id,
            );
        }
        None => println!("[2/3] No commentary item found in catalog."),
    }

    // 4. Locate `theographic:person` entity set.
    match pick_people_entity_set(&store) {
        Some(people_set) => {
            println!(
                "[3/3] People entity set: '{}' ({})",
                people_set.name, people_set.id,
            );
        }
        None => println!("[3/3] Entity set 'theographic:person' not found."),
    }

    Ok(())
}

fn main() {
    let engine: Arc<XBibleEngine> = XBibleEngine::new();
    let db_path: PathBuf = fresh_store_db_path();
    println!("Using fresh temporary database at: {}", db_path.display());

    let db_path_str: String = db_path.to_string_lossy().into_owned();
    if let Err(err) = engine.set_db_path(db_path_str) {
        eprintln!("Failed to open database {}: {err:?}", db_path.display());
        std::process::exit(1);
    }

    let explicit_bible_id: Option<String> = std::env::args().nth(1);

    if let Err(err) = install_three_items(&engine, explicit_bible_id.as_deref()) {
        eprintln!("Error: {err}");
        std::process::exit(1);
    }
}
