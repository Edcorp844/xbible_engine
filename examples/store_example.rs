use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use log::{debug, error, info};
use xbible_engine::XBibleEngine;
use xbible_engine::features::store::{Store, StoreItem, StoreTaskState};

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
                error!("Warning: could not remove {}: {err}", candidate.display());
            }
        }
    }
}

fn fresh_store_db_path() -> PathBuf {
    let dir: PathBuf = std::env::temp_dir().join("xbible_store_test");
    if let Err(err) = std::fs::create_dir_all(&dir) {
        error!(
            "Warning: could not create temp dir {}: {err}",
            dir.display()
        );
    }
    let db_path: PathBuf = dir.join("store_testtmt.db");
    remove_sqlite_file_and_sidecars(&db_path);
    db_path
}

fn poll_until_done(store: &Arc<Store>, task_id: String) -> Result<(), String> {
    let mut last_line: Option<String> = None;

    loop {
        let status = store.poll_task(task_id.clone());
        match status {
            Some(s) => match s.state {
                StoreTaskState::Queued | StoreTaskState::Running => {
                    let line = format!(
                        "  [{:.0}%] {}",
                        s.progress.fraction * 100.0,
                        s.progress.message
                    );
                    if last_line.as_ref() != Some(&line) {
                        info!("{line}");
                        last_line = Some(line);
                    }
                }
                StoreTaskState::Completed => {
                    info!("  Completed: {}", s.progress.message);
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
    store: &Arc<Store>,
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
                    !item.id.eq_ignore_ascii_case("BSB") && item.id.eq_ignore_ascii_case(requested)
                })
        {
            return Ok(found);
        }
    }

    for candidate_id in ["WEB", "ASV", "KJV", "YLT", "DBY"] {
        if let Some(found) = store
            .search(candidate_id.to_string())
            .into_iter()
            .find(|item: &StoreItem| !item.id.eq_ignore_ascii_case("BSB"))
        {
            return Ok(found);
        }
    }

    store
        .search("".to_string())
        .into_iter()
        .find(|item: &StoreItem| !item.id.eq_ignore_ascii_case("BSB"))
        .ok_or_else(|| "no non-BSB translation found in store catalog".to_string())
}

fn pick_commentary(store: &Arc<Store>) -> Option<StoreItem> {
    store
        .items_by_category("Commentaries".to_string(), None)
        .into_iter()
        .find(|item: &StoreItem| item.id == "adam-clarke")
        .or_else(|| {
            store
                .items_by_category("Commentaries".to_string(), None)
                .into_iter()
                .next()
        })
}

fn pick_people_item(store: &Arc<Store>) -> Option<StoreItem> {
    store
        .search("".to_string())
        .into_iter()
        .find(|item: &StoreItem| {
            let id_l = item.id.to_ascii_lowercase();
            let name_l = item.name.to_ascii_lowercase();
            let eng_l = item.english_name.to_ascii_lowercase();
            item.id == "theographic"
                || (item.kind == "entity_set"
                    && (id_l.contains("theographic")
                        || name_l.contains("theographic")
                        || eng_l.contains("theographic")))
        })
}

pub fn install_three_items(
    engine: &Arc<XBibleEngine>,
    explicit_bible_id: Option<&str>,
) -> Result<(), String> {
    let store: Arc<Store> = engine.store();

    store
        .refresh()
        .map_err(|e| format!("refresh failed: {e:?}"))?;

    // 1. Non-BSB Bible translation
    let bible: StoreItem = pick_non_bsb_translation(&store, explicit_bible_id)?;
    debug!("[1/3] Installing '{}' ({})", bible.name, bible.id);
    let bible_task: String = store
        .install(bible)
        .map_err(|e| format!("bible install start failed: {e:?}"))?;
    poll_until_done(&store, bible_task)?;

    // 2. Commentary
    if let Some(commentary) = pick_commentary(&store) {
        debug!("[2/3] Installing '{}' ({})", commentary.name, commentary.id);
        let comm_task: String = store
            .install(commentary)
            .map_err(|e| format!("commentary install start failed: {e:?}"))?;
        poll_until_done(&store, comm_task)?;
    } else {
        error!("[2/3] No commentary item found.");
    }

    // 3. People item (`theographic:person` or matching entity_set/dataset)
    // 3. People item (`theographic:person` or matching entity_set/dataset)
    if let Some(people_item) = pick_people_item(&store) {
        debug!(
            "[3/3] Installing '{}' ({})",
            people_item.name, people_item.id
        );
        let ent_task: String = store
            .install(people_item)
            .map_err(|e| format!("people install start failed: {e:?}"))?;
        poll_until_done(&store, ent_task)?;
    } else {
        error!("[3/3] People item not found. Catalog non-bible/commentary items:");
        for item in store.search("".to_string()) {
            if item.kind != "translation" && item.kind != "bible" && item.kind != "commentary" {
                debug!(
                    "  - id='{}' kind='{}' name='{}' eng='{}'",
                    item.id, item.kind, item.name, item.english_name
                );
            }
        }
    }

    Ok(())
}

fn main() {
    let engine: Arc<XBibleEngine> = XBibleEngine::new();
    let db_path: PathBuf = fresh_store_db_path();
    info!("Using fresh temporary database at: {}", db_path.display());

    let db_path_str: String = db_path.to_string_lossy().into_owned();
    if let Err(err) = engine.set_db_path(db_path_str) {
        error!("Failed to open database {}: {err:?}", db_path.display());
        std::process::exit(1);
    }

    let explicit_bible_id: Option<String> = std::env::args().nth(1);

    if let Err(err) = install_three_items(&engine, explicit_bible_id.as_deref()) {
        error!("Error: {err}");
        std::process::exit(1);
    }
}
