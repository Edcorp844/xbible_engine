use xbible_engine::engines::freebible_engine::BibleClient;
use xbible_engine::engines::freebible_engine::store_api::FreeBibleStore;

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

const DATASET: &str = "theographic";

fn install_kind<F>(
    client: &BibleClient,
    conn: &mut rusqlite::Connection,
    kind: xbible_engine::engines::freebible_engine::install::EntityKind,
    label: &str,
    step_percent: f64,
    mut on_progress: F,
) -> Result<String, xbible_engine::engines::freebible_engine::utils::FreeBibleError>
where
    F: FnMut(f64, String),
{
    let cancel = Arc::new(AtomicBool::new(false));
    let last_pct = Cell::new(-1.0f64);

    xbible_engine::engines::freebible_engine::install::install_entity_set(
        client,
        conn,
        DATASET,
        kind,
        |p| {
            let pct = p.fraction * 100.0;
            let prev = last_pct.get();
            if pct - prev >= step_percent || pct >= 100.0 {
                last_pct.set(pct);
                on_progress(pct, p.message);
            }
        },
        cancel,
    )
    .map(|id| {
        println!("  {} install result: Ok({})", label, id);
        id
    })
    .map_err(|e| {
        println!("  {} install failed: {:?}", label, e);
        e
    })
}

fn main() {
    let client = BibleClient::new();
    use xbible_engine::engines::freebible_engine::install::EntityKind;

    // ── Fresh DB ─────────────────────────────────────────────────
    let db_path = std::env::temp_dir().join("entity_smoke.db");
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));
    let db_path_str = db_path.to_string_lossy().into_owned();
    let mut conn = xbible_engine::db::open(&db_path_str).expect("open db");

    // ── Install all four entity kinds ────────────────────────────
    println!("=== Installing Bible Characters (3067 people) ===");
    let _ = install_kind(&client, &mut conn, EntityKind::Person, "people", 10.0, |pct, msg| {
        println!("  [{:>5.1}%] {}", pct, msg);
    });

    println!("\n=== Installing Bible Events (450 events) ===");
    let _ = install_kind(&client, &mut conn, EntityKind::Event, "events", 10.0, |pct, msg| {
        println!("  [{:>5.1}%] {}", pct, msg);
    });

    println!("\n=== Installing Bible Places (1274 places) ===");
    let _ = install_kind(&client, &mut conn, EntityKind::Place, "places", 10.0, |pct, msg| {
        println!("  [{:>5.1}%] {}", pct, msg);
    });

    println!("\n=== Installing People Groups (23 groups) ===");
    let _ = install_kind(&client, &mut conn, EntityKind::Group, "groups", 20.0, |pct, msg| {
        println!("  [{:>5.1}%] {}", pct, msg);
    });

    // ── Fresh store on the same DB, for reads ────────────────────
    let store = FreeBibleStore::new();
    store.set_db_path(db_path_str.clone()).expect("set db path");

    // ── Row counts + install state ───────────────────────────────
    println!("\n=== Install state ===");
    for kind in ["person", "event", "place", "group"] {
        let installed = store.is_entity_set_installed(DATASET.into(), kind.into());
        println!("  {}: installed={}", kind, installed);
    }

    // ── Browse: people ───────────────────────────────────────────
    println!("\n=== Browse people ===");
    let first_people = store.list_people(DATASET.into(), 0, 5, None);
    println!("  first 5 people:");
    for p in &first_people {
        println!("    {} ({}) refs={}", p.name, p.id, p.number_of_references);
    }

    let search = store.list_people(DATASET.into(), 0, 10, Some("joseph".into()));
    println!("\n  search 'joseph' → {} results:", search.len());
    for p in &search {
        println!("    {} ({})", p.name, p.id);
    }

    // ── Browse: events ───────────────────────────────────────────
    println!("\n=== Browse events ===");
    let some_events = store.list_events(DATASET.into(), 0, 5, None);
    for e in &some_events {
        println!(
            "    {} ({}) start={:?}",
            e.name, e.id, e.start_date
        );
    }

    // ── Browse: places ───────────────────────────────────────────
    println!("\n=== Browse places ===");
    let some_places = store.list_places(DATASET.into(), 0, 5, None);
    for p in &some_places {
        println!("    {} ({})", p.name, p.id);
    }

    // ── Browse: groups ───────────────────────────────────────────
    println!("\n=== Browse groups ===");
    let all_groups = store.list_groups(DATASET.into(), 0, 50, None);
    for g in &all_groups {
        println!("    {} ({})", g.name, g.id);
    }

    // ── Global search ────────────────────────────────────────────
    println!("\n=== Global search 'jesus' ===");
    let hits = store.search_entities(DATASET.into(), "jesus".into());
    for h in hits.iter().take(10) {
        println!("    {} ({})", h.name, h.id);
    }

    // ── Person detail: Joseph ────────────────────────────────────
    println!("\n=== Joseph (joseph_1710) ===");
    let Some(joseph) = store.get_person(DATASET.into(), "joseph_1710".into()) else {
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
    println!(
        "  events (forward): {} refs",
        joseph.events.len()
    );
    println!(
        "  events (reverse): {} refs",
        joseph.referenced_by_events.len()
    );
    println!("  verses: {} ranges", joseph.verses.len());

    // ── Event detail: 7 years of famine ──────────────────────────
    println!("\n=== 7 years of famine (7-years-of-famine_113) ===");
    let Some(famine) = store.get_event(DATASET.into(), "7-years-of-famine_113".into()) else {
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
    println!(
        "  predecessor: {:?}",
        famine.predecessor.as_ref().map(|r| &r.name)
    );
    println!(
        "  successor: {:?}",
        famine.successor.as_ref().map(|r| &r.name)
    );
    println!("  verses: {} ranges", famine.verses.len());

    // ── Place detail: Nazareth ───────────────────────────────────
    println!("\n=== Nazareth (nazareth_878) ===");
    let Some(nazareth) = store.get_place(DATASET.into(), "nazareth_878".into()) else {
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
    println!(
        "  people_born_here: {}",
        nazareth.people_born_here.len()
    );
    println!(
        "  people_died_here: {}",
        nazareth.people_died_here.len()
    );
    println!("  verses: {} ranges", nazareth.verses.len());

    // ── Group detail ─────────────────────────────────────────────
    println!("\n=== Tribe of Joseph ===");
    if let Some(group) = store.get_group(DATASET.into(), "tribe-of-joseph".into()) {
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

    // ── Graph walk: Joseph → event → participants ────────────────
    println!("\n=== Bidirectional graph walk ===");
    if let Some(first_event) = joseph.events.first() {
        println!(
            "  Joseph → event: {} ({})",
            first_event.name, first_event.id
        );
        if let Some(ev) = store.get_event(DATASET.into(), first_event.id.clone()) {
            println!("  event → {} participants", ev.participants.len());
            println!("  event → {} verse ranges", ev.verses.len());
        }
    }

    // ── get_related with relation filter ─────────────────────────
    println!("\n=== get_related (filtered) ===");
    let joseph_participates_in = store.get_related(
        DATASET.into(),
        "person".into(),
        "joseph_1710".into(),
        Some("participated_in".into()),
    );
    println!(
        "  Joseph participated_in: {} events",
        joseph_participates_in.len()
    );

    let joseph_all = store.get_related(
        DATASET.into(),
        "person".into(),
        "joseph_1710".into(),
        None,
    );
    println!(
        "  Joseph all outgoing edges: {} (family + events + groups + places)",
        joseph_all.len()
    );

    // ── get_verse_refs explicitly ────────────────────────────────
    println!("\n=== get_verse_refs ===");
    let joseph_verses = store.get_verse_refs(
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
            v.end_verse
                .map(|e| format!("-{e}"))
                .unwrap_or_default()
        );
    }

    // ── The killer query: who's in this verse ────────────────────
    println!("\n=== Entities in GEN 41:54 ===");
    let entities = store.get_entities_for_verse(
        DATASET.into(),
        "GEN".into(),
        41,
        54,
    );
    for e in &entities {
        println!("  {}: {} ({})", e.kind, e.name, e.id);
    }

    println!("\n=== Entities in JHN 3:16 ===");
    let entities = store.get_entities_for_verse(
        DATASET.into(),
        "JHN".into(),
        3,
        16,
    );
    for e in &entities {
        println!("  {}: {} ({})", e.kind, e.name, e.id);
    }

    // ── Cleanup ──────────────────────────────────────────────────
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_file(format!("{}-wal", db_path.display()));
    let _ = std::fs::remove_file(format!("{}-shm", db_path.display()));

    println!("\nDone.");
}