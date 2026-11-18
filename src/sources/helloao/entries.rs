use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use log::{error, warn};
use rusqlite::{Connection, params};
use tokio::sync::Semaphore;
use tokio::task::{JoinError, JoinSet};

use crate::error::XBibleError;
use crate::runtime::get_shared_runtime;
use crate::sources::InstallProgressSink;
use crate::sources::helloao::client::{HelloAOClient, HelloEntitySummary};

const MAX_CONCURRENT_ENTITY_FETCHES: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    Person,
    Event,
    Place,
    Group,
}

impl EntityKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EntityKind::Person => "person",
            EntityKind::Event => "event",
            EntityKind::Place => "place",
            EntityKind::Group => "group",
        }
    }

    pub fn object_key(&self) -> &'static str {
        match self {
            EntityKind::Person => "person",
            EntityKind::Event => "event",
            EntityKind::Place => "place",
            EntityKind::Group => "group",
        }
    }

    pub fn table_name(&self) -> &'static str {
        match self {
            EntityKind::Person => "people",
            EntityKind::Event => "events",
            EntityKind::Place => "places",
            EntityKind::Group => "people_groups",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FetchedEntityPayload {
    pub kind: EntityKind,
    pub raw_value: serde_json::Value,
}

#[derive(Debug, Clone)]
pub struct FetchedEntityDataset {
    pub dataset_id: String,
    pub entities: Vec<FetchedEntityPayload>,
}

fn join_description(val: &serde_json::Value) -> Option<String> {
    match val {
        serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
        serde_json::Value::Array(arr) => {
            let parts: Vec<&str> = arr
                .iter()
                .filter_map(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .collect();
            if parts.is_empty() {
                None
            } else {
                Some(parts.join("\n"))
            }
        }
        _ => None,
    }
}

pub fn fetch_entity_dataset(
    client: &HelloAOClient,
    dataset_id: &str,
    progress: &dyn InstallProgressSink,
) -> Result<FetchedEntityDataset, String> {
    if progress.is_cancelled() {
        return Err("Entity dataset download cancelled".to_string());
    }

    progress.on_progress(
        "catalog".to_string(),
        0,
        4,
        Some(format!("Fetching entity lists for {dataset_id}")),
    );

    let lists: [(EntityKind, Vec<HelloEntitySummary>); 4] = [
        (
            EntityKind::Person,
            client.dataset_people_list(dataset_id).unwrap_or_default(),
        ),
        (
            EntityKind::Event,
            client.dataset_events_list(dataset_id).unwrap_or_default(),
        ),
        (
            EntityKind::Place,
            client.dataset_places_list(dataset_id).unwrap_or_default(),
        ),
        (
            EntityKind::Group,
            client.dataset_groups_list(dataset_id).unwrap_or_default(),
        ),
    ];

    let mut tasks: Vec<(EntityKind, String)> = Vec::new();
    for (kind, summaries) in lists {
        for s in summaries {
            if !s.id.is_empty() {
                tasks.push((kind, s.id));
            }
        }
    }

    let total: u32 = tasks.len() as u32;
    if total == 0 {
        return Err(format!("no entities found in dataset {dataset_id}"));
    }

    progress.on_progress(
        "download".to_string(),
        0,
        total,
        Some(format!("Downloading {total} entities from {dataset_id}")),
    );

    let done_counter: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));
    let counter_for_loop = Arc::clone(&done_counter);
    let runtime = get_shared_runtime();
    let semaphore: Arc<Semaphore> = Arc::new(Semaphore::new(MAX_CONCURRENT_ENTITY_FETCHES));
    let client_arc: Arc<HelloAOClient> = Arc::new(client.clone());
    let dataset_arc: Arc<String> = Arc::new(dataset_id.to_string());

    let entities: Vec<FetchedEntityPayload> = runtime.block_on(async move {
        let mut join_set: JoinSet<Option<FetchedEntityPayload>> = JoinSet::new();

        for (kind, entity_id) in tasks {
            if progress.is_cancelled() {
                break;
            }
            let permit = match Arc::clone(&semaphore).acquire_owned().await {
                Ok(p) => p,
                Err(_) => break,
            };
            let client_ref = Arc::clone(&client_arc);
            let ds_ref = Arc::clone(&dataset_arc);

            join_set.spawn(async move {
                let _permit = permit;
                let ds = (*ds_ref).clone();
                let eid = entity_id.clone();

                let res: Result<serde_json::Value, String> =
                    tokio::task::spawn_blocking(move || match kind {
                        EntityKind::Person => client_ref.dataset_person(&ds, &eid),
                        EntityKind::Event => client_ref.dataset_event(&ds, &eid),
                        EntityKind::Place => client_ref.dataset_place(&ds, &eid),
                        EntityKind::Group => client_ref.dataset_group(&ds, &eid),
                    })
                    .await
                    .unwrap_or_else(|e: JoinError| Err(e.to_string()));

                match res {
                    Ok(raw_value) => Some(FetchedEntityPayload { kind, raw_value }),
                    Err(err) => {
                        error!("fetch {} {} failed: {}", kind.as_str(), entity_id, err);
                        None
                    }
                }
            });
        }

        let mut out: Vec<FetchedEntityPayload> = Vec::with_capacity(total as usize);
        while let Some(joined) = join_set.join_next().await {
            if let Ok(Some(payload)) = joined {
                out.push(payload);
                let done: u32 = counter_for_loop.fetch_add(1, Ordering::Relaxed) + 1;
                progress.on_progress(
                    "download".to_string(),
                    done,
                    total,
                    Some(format!("Downloaded {done}/{total} entities")),
                );
            }
            if progress.is_cancelled() {
                join_set.abort_all();
                break;
            }
        }
        out
    });

    if progress.is_cancelled() {
        return Err("Entity dataset installation cancelled".to_string());
    }

    progress.on_progress(
        "normalize".to_string(),
        0,
        entities.len() as u32,
        Some(format!("Preparing {} entities for install", entities.len())),
    );

    Ok(FetchedEntityDataset {
        dataset_id: dataset_id.to_string(),
        entities,
    })
}

pub fn begin_entity_install(
    conn: &Connection,
    dataset_id: &str,
    kind: EntityKind,
) -> Result<(), XBibleError> {
    let table: &str = kind.table_name();
    conn.execute(
        &format!("DELETE FROM {table} WHERE dataset_id = ?1"),
        params![dataset_id],
    )
    .map_err(|e| XBibleError::Generic {
        error_message: format!("wipe {table}: {e}"),
    })?;
    conn.execute(
        "DELETE FROM entity_refs WHERE dataset_id = ?1 AND source_kind = ?2",
        params![dataset_id, kind.as_str()],
    )
    .map_err(|e| XBibleError::Generic {
        error_message: format!("wipe entity_refs: {e}"),
    })?;
    conn.execute(
        "DELETE FROM entity_verse_refs WHERE dataset_id = ?1 AND source_kind = ?2",
        params![dataset_id, kind.as_str()],
    )
    .map_err(|e| XBibleError::Generic {
        error_message: format!("wipe entity_verse_refs: {e}"),
    })?;
    Ok(())
}

pub fn persist_entity_dataset(
    conn: &Connection,
    dataset: &FetchedEntityDataset,
    on_entity_progress: &mut impl FnMut(usize, usize, &str),
) -> Result<(), XBibleError> {
    for kind in [
        EntityKind::Person,
        EntityKind::Event,
        EntityKind::Place,
        EntityKind::Group,
    ] {
        begin_entity_install(conn, &dataset.dataset_id, kind)?;
    }

    let total: usize = dataset.entities.len();
    let mut counts: [i32; 4] = [0; 4];

    for (idx, item) in dataset.entities.iter().enumerate() {
        let key: &str = item.kind.object_key();
        let Some(obj) = item.raw_value.get(key) else {
            warn!(
                "{} response missing `{}` field — skipping",
                item.kind.as_str(),
                key
            );
            continue;
        };
        let raw_json: String = serde_json::to_string(&item.raw_value).unwrap_or_default();

        let entity_id: String = match item.kind {
            EntityKind::Person => {
                counts[0] += 1;
                write_person(conn, &dataset.dataset_id, obj, &raw_json)?
            }
            EntityKind::Event => {
                counts[1] += 1;
                write_event(conn, &dataset.dataset_id, obj, &raw_json)?
            }
            EntityKind::Place => {
                counts[2] += 1;
                write_place(conn, &dataset.dataset_id, obj, &raw_json)?
            }
            EntityKind::Group => {
                counts[3] += 1;
                write_group(conn, &dataset.dataset_id, obj, &raw_json)?
            }
        };

        let label: &str = obj
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&entity_id);
        on_entity_progress(idx + 1, total, label);
    }

    for (i, kind) in [
        EntityKind::Person,
        EntityKind::Event,
        EntityKind::Place,
        EntityKind::Group,
    ]
    .iter()
    .enumerate()
    {
        mark_entity_installed(conn, &dataset.dataset_id, *kind, counts[i], counts[i])?;
    }

    Ok(())
}

fn mark_entity_installed(
    conn: &Connection,
    dataset_id: &str,
    kind: EntityKind,
    fetched: i32,
    total: i32,
) -> Result<(), XBibleError> {
    conn.execute(
        "INSERT OR REPLACE INTO entity_install_state
         (dataset_id, kind, installed, fetched_count, total_count)
         VALUES (?1, ?2, 1, ?3, ?4)",
        params![dataset_id, kind.as_str(), fetched, total],
    )
    .map_err(|e| XBibleError::Generic {
        error_message: format!("entity_install_state: {e}"),
    })?;
    Ok(())
}

fn write_person(
    conn: &Connection,
    dataset_id: &str,
    person: &serde_json::Value,
    raw_json: &str,
) -> Result<String, XBibleError> {
    let id: String = person
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let name: &str = person
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let also_called: Option<String> =
        person
            .get("alsoCalled")
            .and_then(|v| v.as_array())
            .map(|arr| {
                let names: Vec<&str> = arr.iter().filter_map(|v| v.as_str()).collect();
                serde_json::to_string(&names).unwrap_or_default()
            });
    let is_proper_name: bool = person
        .get("isProperName")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let gender: Option<&str> = person.get("gender").and_then(|v| v.as_str());
    let birth_year: Option<i64> = person.get("birthYear").and_then(|v| v.as_i64());
    let death_year: Option<i64> = person.get("deathYear").and_then(|v| v.as_i64());
    let min_year: Option<i64> = person.get("minYear").and_then(|v| v.as_i64());
    let max_year: Option<i64> = person.get("maxYear").and_then(|v| v.as_i64());
    let description: Option<String> = join_description(
        person
            .get("description")
            .unwrap_or(&serde_json::Value::Null),
    );

    conn.execute(
        "INSERT OR REPLACE INTO people (
            dataset_id, id, name, also_called, is_proper_name, gender,
            birth_year, death_year, min_year, max_year, description, raw_json
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            dataset_id,
            id,
            name,
            also_called,
            is_proper_name as i32,
            gender,
            birth_year,
            death_year,
            min_year,
            max_year,
            description,
            raw_json,
        ],
    )
    .map_err(|e| XBibleError::Generic {
        error_message: format!("people insert: {e}"),
    })?;

    Ok(id)
}

fn write_event(
    conn: &Connection,
    dataset_id: &str,
    event: &serde_json::Value,
    raw_json: &str,
) -> Result<String, XBibleError> {
    let id: String = event
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let name: &str = event
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let start_date: Option<&str> = event.get("startDate").and_then(|v| v.as_str());
    let end_date: Option<&str> = event.get("endDate").and_then(|v| v.as_str());
    let duration: Option<&str> = event.get("duration").and_then(|v| v.as_str());
    let description: Option<String> =
        join_description(event.get("description").unwrap_or(&serde_json::Value::Null));

    conn.execute(
        "INSERT OR REPLACE INTO events (
            dataset_id, id, name, start_date, end_date, duration, description, raw_json
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            dataset_id,
            id,
            name,
            start_date,
            end_date,
            duration,
            description,
            raw_json
        ],
    )
    .map_err(|e| XBibleError::Generic {
        error_message: format!("events insert: {e}"),
    })?;

    Ok(id)
}

fn write_place(
    conn: &Connection,
    dataset_id: &str,
    place: &serde_json::Value,
    raw_json: &str,
) -> Result<String, XBibleError> {
    let id: String = place
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let name: &str = place
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let kjv_name: Option<&str> = place.get("kjvName").and_then(|v| v.as_str());
    let esv_name: Option<&str> = place.get("esvName").and_then(|v| v.as_str());
    let feature_type: Option<&str> = place.get("featureType").and_then(|v| v.as_str());
    let latitude: Option<f64> = place.get("latitude").and_then(|v| v.as_f64());
    let longitude: Option<f64> = place.get("longitude").and_then(|v| v.as_f64());
    let description: Option<String> =
        join_description(place.get("description").unwrap_or(&serde_json::Value::Null));

    conn.execute(
        "INSERT OR REPLACE INTO places (
            dataset_id, id, name, kjv_name, esv_name, feature_type,
            latitude, longitude, description, raw_json
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![
            dataset_id,
            id,
            name,
            kjv_name,
            esv_name,
            feature_type,
            latitude,
            longitude,
            description,
            raw_json,
        ],
    )
    .map_err(|e| XBibleError::Generic {
        error_message: format!("places insert: {e}"),
    })?;

    Ok(id)
}

fn write_group(
    conn: &Connection,
    dataset_id: &str,
    group: &serde_json::Value,
    raw_json: &str,
) -> Result<String, XBibleError> {
    let id: String = group
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let name: &str = group
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let description: Option<String> =
        join_description(group.get("description").unwrap_or(&serde_json::Value::Null));

    conn.execute(
        "INSERT OR REPLACE INTO people_groups (
            dataset_id, id, name, description, raw_json
         ) VALUES (?1,?2,?3,?4,?5)",
        params![dataset_id, id, name, description, raw_json],
    )
    .map_err(|e| XBibleError::Generic {
        error_message: format!("people_groups insert: {e}"),
    })?;

    Ok(id)
}
