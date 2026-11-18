use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use log::{error, info, warn};
use xbible_engine::XBibleEngine;
use xbible_engine::features::store::{Store, StoreItem, StoreTaskState};

const DATASET_HINT: &str = "theographic";

fn cleanup(path: &Path) {
    let base = path.to_string_lossy();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let candidate = PathBuf::from(format!("{base}{suffix}"));
        if candidate.exists() {
            if let Err(err) = std::fs::remove_file(&candidate) {
                warn!("Could not remove {}: {err}", candidate.display());
            }
        }
    }
}

fn fresh_db_path() -> PathBuf {
    let dir = std::env::temp_dir().join("xbible_entities_test");
    if let Err(err) = std::fs::create_dir_all(&dir) {
        warn!("Could not create temp dir {}: {err}", dir.display());
    }
    let db_path = dir.join(format!("entity_reader_{}.db", std::process::id()));
    cleanup(&db_path);
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

fn pick_entity_dataset(store: &Arc<Store>, requested: &str) -> Result<StoreItem, String> {
    let candidates = store.search(requested.to_string());
    if let Some(item) = candidates.into_iter().find(|item: &StoreItem| {
        item.id.eq_ignore_ascii_case(requested)
            || item
                .id
                .to_ascii_lowercase()
                .contains(&requested.to_ascii_lowercase())
            || item.kind.contains("entity")
            || item.kind.contains("dataset")
    }) {
        return Ok(item);
    }

    store
        .search("".to_string())
        .into_iter()
        .find(|item: &StoreItem| {
            let kind_l = item.kind.to_ascii_lowercase();
            let id_l = item.id.to_ascii_lowercase();
            id_l.contains("theographic") || kind_l.contains("entity") || kind_l.contains("dataset")
        })
        .ok_or_else(|| format!("No entity dataset matching '{requested}' found in store"))
}

fn run_entity_example(engine: &Arc<XBibleEngine>, dataset_filter: &str) -> Result<(), String> {
    let store: Arc<Store> = engine.store();
    store
        .refresh()
        .map_err(|e| format!("store refresh failed: {e:?}"))?;

    let dataset_item = pick_entity_dataset(&store, dataset_filter)?;
    let dataset_id = dataset_item.id.clone();

    info!(
        "=== Installing entity dataset '{}' ({}, kind='{}') ===",
        dataset_item.name, dataset_id, dataset_item.kind
    );
    let task_id = store
        .install(dataset_item)
        .map_err(|e| format!("entity install start failed: {e:?}"))?;
    poll_until_done(&store, task_id)?;

    let reader = engine.reader();

    // ── Install state ───────────────────────────────────────────
    info!("=== Install state ===");
    for kind in ["person", "event", "place", "group"] {
        let installed = reader.is_entity_set_installed(dataset_id.clone(), kind.into());
        info!("  {kind}: installed={installed}");
    }

    // ── Browse: people ─────────────────────────────────────────
    info!("=== Browse people ===");
    let first_people = reader.list_people(dataset_id.clone(), 0, 5, None);
    info!("  first 5 people:");
    for p in &first_people {
        info!("    {} ({}) refs={}", p.name, p.id, p.number_of_references);
    }

    let joseph_matches = reader.list_people(dataset_id.clone(), 0, 10, Some("joseph".into()));
    info!("  search 'joseph' -> {} results:", joseph_matches.len());
    for p in &joseph_matches {
        info!("    {} ({})", p.name, p.id);
    }

    // ── Browse: events ─────────────────────────────────────────
    info!("=== Browse events ===");
    let some_events = reader.list_events(dataset_id.clone(), 0, 5, None);
    for e in &some_events {
        info!("    {} ({}) start={:?}", e.name, e.id, e.start_date);
    }

    // ── Browse: places ─────────────────────────────────────────
    info!("=== Browse places ===");
    let some_places = reader.list_places(dataset_id.clone(), 0, 5, None);
    for p in &some_places {
        info!("    {} ({})", p.name, p.id);
    }

    // ── Browse: groups ─────────────────────────────────────────
    info!("=== Browse groups ===");
    let all_groups = reader.list_groups(dataset_id.clone(), 0, 50, None);
    for g in &all_groups {
        info!("    {} ({})", g.name, g.id);
    }

    // ── Global search ──────────────────────────────────────────
    info!("=== Global search 'jesus' ===");
    let hits = reader.search_entities(dataset_id.clone(), "jesus".into());
    for h in hits.iter().take(10) {
        info!("    {} ({})", h.name, h.id);
    }

    // ── Person detail: Joseph ───────────────────────────────────
    info!("=== Joseph (joseph_1710) ===");
    if let Some(joseph) = reader.get_person(dataset_id.clone(), "joseph_1710".into()) {
        info!("  name: {}", joseph.name);
        info!("  also_called: {:?}", joseph.also_called);
        info!("  gender: {:?}", joseph.gender);
        info!("  birth_year: {:?}", joseph.birth_year);
        info!("  death_year: {:?}", joseph.death_year);
        info!(
            "  description length: {}",
            joseph.description.as_deref().map(|s| s.len()).unwrap_or(0)
        );
        info!(
            "  father: {:?}",
            joseph.father.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
        info!(
            "  mother: {:?}",
            joseph.mother.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
        info!(
            "  partners: {:?}",
            joseph.partners.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
        info!(
            "  children: {:?}",
            joseph.children.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
        info!(
            "  siblings: {:?}",
            joseph.siblings.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
        info!(
            "  half_siblings_same_father: {} refs",
            joseph.half_siblings_same_father.len()
        );
        info!(
            "  member_of: {:?}",
            joseph.member_of.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
        info!(
            "  birth_place: {:?}",
            joseph
                .birth_place
                .iter()
                .map(|r| &r.name)
                .collect::<Vec<_>>()
        );
        info!(
            "  death_place: {:?}",
            joseph
                .death_place
                .iter()
                .map(|r| &r.name)
                .collect::<Vec<_>>()
        );
        info!("  events (forward): {} refs", joseph.events.len());
        info!(
            "  events (reverse): {} refs",
            joseph.referenced_by_events.len()
        );
        info!("  verses: {} ranges", joseph.verses.len());

        // ── Graph walk ─────────────────────────────────────────
        info!("=== Bidirectional graph walk ===");
        if let Some(first_event) = joseph.events.first() {
            info!(
                "  Joseph -> event: {} ({})",
                first_event.name, first_event.id
            );
            if let Some(ev) = reader.get_event(dataset_id.clone(), first_event.id.clone()) {
                info!("  event -> {} participants", ev.participants.len());
                info!("  event -> {} verse ranges", ev.verses.len());
            }
        }
    } else {
        warn!("  Joseph (joseph_1710) not found");
    }

    // ── Event detail ───────────────────────────────────────────
    info!("=== 7 years of famine (7-years-of-famine_113) ===");
    if let Some(famine) = reader.get_event(dataset_id.clone(), "7-years-of-famine_113".into()) {
        info!("  name: {}", famine.name);
        info!("  start_date: {:?}", famine.start_date);
        info!("  duration: {:?}", famine.duration);
        info!("  participants: {}", famine.participants.len());
        for p in famine.participants.iter().take(5) {
            info!("    {} ({})", p.name, p.id);
        }
        if famine.participants.len() > 5 {
            info!("    ... {} more", famine.participants.len() - 5);
        }
        info!("  locations: {}", famine.locations.len());
        info!(
            "  predecessor: {:?}",
            famine.predecessor.as_ref().map(|r| &r.name)
        );
        info!(
            "  successor: {:?}",
            famine.successor.as_ref().map(|r| &r.name)
        );
        info!("  verses: {} ranges", famine.verses.len());
    } else {
        warn!("  Event (7-years-of-famine_113) not found");
    }

    // ── Place detail ───────────────────────────────────────────
    info!("=== Nazareth (nazareth_878) ===");
    if let Some(nazareth) = reader.get_place(dataset_id.clone(), "nazareth_878".into()) {
        info!("  name: {}", nazareth.name);
        info!("  feature_type: {:?}", nazareth.feature_type);
        info!(
            "  lat/lon: {:?} / {:?}",
            nazareth.latitude, nazareth.longitude
        );
        info!(
            "  description length: {}",
            nazareth
                .description
                .as_deref()
                .map(|s| s.len())
                .unwrap_or(0)
        );
        info!("  events located here: {}", nazareth.events.len());
        for e in nazareth.events.iter().take(5) {
            info!("    {} ({})", e.name, e.id);
        }
        if nazareth.events.len() > 5 {
            info!("    ... {} more", nazareth.events.len() - 5);
        }
        info!("  people_born_here: {}", nazareth.people_born_here.len());
        info!("  people_died_here: {}", nazareth.people_died_here.len());
        info!("  verses: {} ranges", nazareth.verses.len());
    } else {
        warn!("  Place (nazareth_878) not found");
    }

    // ── Group detail ───────────────────────────────────────────
    info!("=== Tribe of Joseph ===");
    if let Some(group) = reader.get_group(dataset_id.clone(), "tribe-of-joseph".into()) {
        info!("  name: {}", group.name);
        info!("  members: {}", group.members.len());
        for m in group.members.iter().take(5) {
            info!("    {} ({})", m.name, m.id);
        }
        if group.members.len() > 5 {
            info!("    ... {} more", group.members.len() - 5);
        }
        info!("  leaders: {}", group.leaders.len());
        info!("  located_at: {}", group.located_at.len());
        info!("  verses: {} ranges", group.verses.len());
    }

    // ── get_related with relation filter ───────────────────────
    info!("=== get_related (filtered) ===");
    let joseph_participates_in = reader.get_related(
        dataset_id.clone(),
        "person".into(),
        "joseph_1710".into(),
        Some("participated_in".into()),
    );
    info!(
        "  Joseph participated_in: {} events",
        joseph_participates_in.len()
    );

    let joseph_all = reader.get_related(
        dataset_id.clone(),
        "person".into(),
        "joseph_1710".into(),
        None,
    );
    info!("  Joseph all outgoing edges: {}", joseph_all.len());

    // ── get_verse_refs explicitly ──────────────────────────────
    info!("=== get_verse_refs ===");
    let joseph_verses =
        reader.get_verse_refs(dataset_id.clone(), "person".into(), "joseph_1710".into());
    info!("  Joseph verse ranges: {}", joseph_verses.len());
    for v in joseph_verses.iter().take(3) {
        info!(
            "    {} {}:{}{}",
            v.book,
            v.chapter,
            v.verse,
            v.end_verse.map(|e| format!("-{e}")).unwrap_or_default()
        );
    }

    // ── Verse-to-entity queries ────────────────────────────────
    info!("=== Entities in GEN 41:54 ===");
    for e in reader.get_entities_for_verse(dataset_id.clone(), "GEN".into(), 41, 54) {
        info!("  {}: {} ({})", e.kind, e.name, e.id);
    }

    info!("=== Entities in JHN 3:16 ===");
    for e in reader.get_entities_for_verse(dataset_id, "JHN".into(), 3, 16) {
        info!("  {}: {} ({})", e.kind, e.name, e.id);
    }

    Ok(())
}

fn main() {
    let engine: Arc<XBibleEngine> = XBibleEngine::new();
    let db_path = fresh_db_path();
    info!("Using fresh temporary database at: {}", db_path.display());

    if let Err(err) = engine.set_db_path(db_path.to_string_lossy().into_owned()) {
        error!("Failed to open database {}: {err:?}", db_path.display());
        std::process::exit(1);
    }

    let dataset_arg = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DATASET_HINT.to_string());

    if let Err(err) = run_entity_example(&engine, &dataset_arg) {
        error!("Error: {err}");
        cleanup(&db_path);
        std::process::exit(1);
    }

    drop(engine);
    cleanup(&db_path);
    info!("Done.");
}

