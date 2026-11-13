use std::thread;
use std::time::Duration;

use xbible_engine::engines::freebible_engine::utils::FreeBibleTaskState;
use xbible_engine::XBibleEngine;

const DATASET: &str = "theographic";

fn main() {
    let db_path = std::env::temp_dir().join(format!(
        "entity_smoke_{}.db",
        std::process::id()
    ));
    cleanup(&db_path);

    let engine = XBibleEngine::new();
    engine
        .freebible()
        .set_db_path(db_path.to_string_lossy().into_owned())
        .expect("engine open failed");

    let store = engine.freebible().store();
    let reader = engine.freebible().reader();

    // ── Install each entity set via the store ───────────────────
    for (kind_str, label) in [
        ("person", "Bible Characters (3067 people)"),
        ("event", "Bible Events (450 events)"),
        ("place", "Bible Places (1274 places)"),
        ("group", "People Groups (23 groups)"),
    ] {
        println!("\n=== Installing {label} ===");
        let task_id = store
            .install_entity_set(DATASET.into(), kind_str.into())
            .expect("install start failed");
        wait_for_task(&store, &task_id);
    }

    // ── Install state ───────────────────────────────────────────
    println!("\n=== Install state ===");
    for kind in ["person", "event", "place", "group"] {
        let installed = reader.is_entity_set_installed(DATASET.into(), kind.into());
        println!("  {}: installed={}", kind, installed);
        assert!(installed, "{kind} should be marked installed");
    }

    // ── Browse: people ─────────────────────────────────────────
    println!("\n=== Browse people ===");
    let first_people = reader.list_people(DATASET.into(), 0, 5, None);
    println!("  first 5 people:");
    for p in &first_people {
        println!("    {} ({}) refs={}", p.name, p.id, p.number_of_references);
    }

    let search = reader.list_people(DATASET.into(), 0, 10, Some("joseph".into()));
    println!("\n  search 'joseph' → {} results:", search.len());
    for p in &search {
        println!("    {} ({})", p.name, p.id);
    }

    // ── Browse: events ─────────────────────────────────────────
    println!("\n=== Browse events ===");
    let some_events = reader.list_events(DATASET.into(), 0, 5, None);
    for e in &some_events {
        println!("    {} ({}) start={:?}", e.name, e.id, e.start_date);
    }

    // ── Browse: places ─────────────────────────────────────────
    println!("\n=== Browse places ===");
    let some_places = reader.list_places(DATASET.into(), 0, 5, None);
    for p in &some_places {
        println!("    {} ({})", p.name, p.id);
    }

    // ── Browse: groups ─────────────────────────────────────────
    println!("\n=== Browse groups ===");
    let all_groups = reader.list_groups(DATASET.into(), 0, 50, None);
    for g in &all_groups {
        println!("    {} ({})", g.name, g.id);
    }

    // ── Global search ──────────────────────────────────────────
    println!("\n=== Global search 'jesus' ===");
    let hits = reader.search_entities(DATASET.into(), "jesus".into());
    for h in hits.iter().take(10) {
        println!("    {} ({})", h.name, h.id);
    }

    // ── Person detail: Joseph ───────────────────────────────────
    println!("\n=== Joseph (joseph_1710) ===");
    let Some(joseph) = reader.get_person(DATASET.into(), "joseph_1710".into()) else {
        println!("  Joseph not found");
        return;
    };
    println!("  name: {}", joseph.name);
    println!("  also_called: {:?}", joseph.also_called);
    println!("  gender: {:?}", joseph.gender);
    println!("  birth_year: {:?}", joseph.birth_year);
    println!("  death_year: {:?}", joseph.death_year);
    println!(
        "  description length: {}",
        joseph.description.as_deref().map(|s| s.len()).unwrap_or(0)
    );

    println!(
        "  father: {:?}",
        joseph.father.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
    println!(
        "  mother: {:?}",
        joseph.mother.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
    println!(
        "  partners: {:?}",
        joseph.partners.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
    println!(
        "  children: {:?}",
        joseph.children.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
    println!(
        "  siblings: {:?}",
        joseph.siblings.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
    println!(
        "  half_siblings_same_father: {} refs",
        joseph.half_siblings_same_father.len()
    );
    println!(
        "  member_of: {:?}",
        joseph.member_of.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
    println!(
        "  birth_place: {:?}",
        joseph.birth_place.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
    println!(
        "  death_place: {:?}",
        joseph.death_place.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
    println!("  events (forward): {} refs", joseph.events.len());
    println!("  events (reverse): {} refs", joseph.referenced_by_events.len());
    println!("  verses: {} ranges", joseph.verses.len());

    // ── Event detail ───────────────────────────────────────────
    println!("\n=== 7 years of famine (7-years-of-famine_113) ===");
    let Some(famine) = reader.get_event(DATASET.into(), "7-years-of-famine_113".into()) else {
        println!("  Event not found");
        return;
    };
    println!("  name: {}", famine.name);
    println!("  start_date: {:?}", famine.start_date);
    println!("  duration: {:?}", famine.duration);
    println!("  participants: {}", famine.participants.len());
    for p in famine.participants.iter().take(5) {
        println!("    {} ({})", p.name, p.id);
    }
    if famine.participants.len() > 5 {
        println!("    … {} more", famine.participants.len() - 5);
    }
    println!("  locations: {}", famine.locations.len());
    println!("  predecessor: {:?}", famine.predecessor.as_ref().map(|r| &r.name));
    println!("  successor: {:?}", famine.successor.as_ref().map(|r| &r.name));
    println!("  verses: {} ranges", famine.verses.len());

    // ── Place detail ───────────────────────────────────────────
    println!("\n=== Nazareth (nazareth_878) ===");
    let Some(nazareth) = reader.get_place(DATASET.into(), "nazareth_878".into()) else {
        println!("  Place not found");
        return;
    };
    println!("  name: {}", nazareth.name);
    println!("  feature_type: {:?}", nazareth.feature_type);
    println!("  lat/lon: {:?} / {:?}", nazareth.latitude, nazareth.longitude);
    println!(
        "  description length: {}",
        nazareth.description.as_deref().map(|s| s.len()).unwrap_or(0)
    );
    println!("  events located here: {}", nazareth.events.len());
    for e in nazareth.events.iter().take(5) {
        println!("    {} ({})", e.name, e.id);
    }
    if nazareth.events.len() > 5 {
        println!("    … {} more", nazareth.events.len() - 5);
    }
    println!("  people_born_here: {}", nazareth.people_born_here.len());
    println!("  people_died_here: {}", nazareth.people_died_here.len());
    println!("  verses: {} ranges", nazareth.verses.len());

    // ── Group detail ───────────────────────────────────────────
    println!("\n=== Tribe of Joseph ===");
    if let Some(group) = reader.get_group(DATASET.into(), "tribe-of-joseph".into()) {
        println!("  name: {}", group.name);
        println!("  members: {}", group.members.len());
        for m in group.members.iter().take(5) {
            println!("    {} ({})", m.name, m.id);
        }
        if group.members.len() > 5 {
            println!("    … {} more", group.members.len() - 5);
        }
        println!("  leaders: {}", group.leaders.len());
        println!("  located_at: {}", group.located_at.len());
        println!("  verses: {} ranges", group.verses.len());
    }

    // ── Graph walk ─────────────────────────────────────────────
    println!("\n=== Bidirectional graph walk ===");
    if let Some(first_event) = joseph.events.first() {
        println!("  Joseph → event: {} ({})", first_event.name, first_event.id);
        if let Some(ev) = reader.get_event(DATASET.into(), first_event.id.clone()) {
            println!("  event → {} participants", ev.participants.len());
            println!("  event → {} verse ranges", ev.verses.len());
        }
    }

    // ── get_related with relation filter ───────────────────────
    println!("\n=== get_related (filtered) ===");
    let joseph_participates_in = reader.get_related(
        DATASET.into(),
        "person".into(),
        "joseph_1710".into(),
        Some("participated_in".into()),
    );
    println!("  Joseph participated_in: {} events", joseph_participates_in.len());

    let joseph_all = reader.get_related(
        DATASET.into(),
        "person".into(),
        "joseph_1710".into(),
        None,
    );
    println!("  Joseph all outgoing edges: {}", joseph_all.len());

    // ── get_verse_refs explicitly ──────────────────────────────
    println!("\n=== get_verse_refs ===");
    let joseph_verses = reader.get_verse_refs(
        DATASET.into(),
        "person".into(),
        "joseph_1710".into(),
    );
    println!("  Joseph verse ranges: {}", joseph_verses.len());
    for v in joseph_verses.iter().take(3) {
        println!(
            "    {} {}:{}{}",
            v.book,
            v.chapter,
            v.verse,
            v.end_verse.map(|e| format!("-{e}")).unwrap_or_default()
        );
    }

    // ── The killer query: who's in this verse ─────────────────
    println!("\n=== Entities in GEN 41:54 ===");
    let entities = reader.get_entities_for_verse(DATASET.into(), "GEN".into(), 41, 54);
    for e in &entities {
        println!("  {}: {} ({})", e.kind, e.name, e.id);
    }

    println!("\n=== Entities in JHN 3:16 ===");
    let entities = reader.get_entities_for_verse(DATASET.into(), "JHN".into(), 3, 16);
    for e in &entities {
        println!("  {}: {} ({})", e.kind, e.name, e.id);
    }

    // ── Cleanup ────────────────────────────────────────────────
    drop(engine);
    cleanup(&db_path);

    println!("\nDone.");
}

// ── Helpers ──────────────────────────────────────────────────────

fn wait_for_task(
    store: &std::sync::Arc<xbible_engine::engines::freebible_engine::store_api::FreeBibleStore>,
    task_id: &str,
) {
    let mut last_pct = -1.0f64;
    loop {
        thread::sleep(Duration::from_millis(300));
        let Some(status) = store.poll_task(task_id.to_string()) else {
            println!("  task disappeared");
            break;
        };
        let pct = status.progress.fraction * 100.0;
        if pct - last_pct >= 10.0 || pct >= 100.0 {
            last_pct = pct;
            println!("  [{:>5.1}%] {}", pct, status.progress.message);
        }
        match status.state {
            FreeBibleTaskState::Running | FreeBibleTaskState::Queued => continue,
            FreeBibleTaskState::Completed => break,
            FreeBibleTaskState::Failed { error } => {
                panic!("install failed: {error}");
            }
            FreeBibleTaskState::Cancelled => panic!("install cancelled"),
        }
    }
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path.to_string_lossy()));
    let _ = std::fs::remove_file(format!("{}-shm", path.to_string_lossy()));
}