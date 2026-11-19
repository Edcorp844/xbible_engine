//! HelloAO source adapter (`src/sources/helloao/mod.rs`).
//!
//! Wraps `HelloAOClient` and implements `BibleSource` so `helloao` endpoint
//! details, raw JSON parsing, and API quirks remain isolated behind normalized
//! `Generic*` structures, while providing explicit installation and
//! uninstallation methods for HelloAO translations and entity datasets[cite: 6, 7].

pub mod catalog;
pub mod client;
pub mod entries;
pub mod values;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use rusqlite::Connection;
use tokio::sync::Semaphore;
use tokio::task::{JoinError, JoinSet};

use crate::db::install::{persist_installed_bible, uninstall_translation};
use crate::error::XBibleError;
use crate::features::store::StoreItem;
use crate::runtime::get_shared_runtime;
use crate::sources::helloao::client::{
    HELLOAO_SOURCE_ID, HelloAOClient, HelloCommentaryMeta, HelloCommentaryProfile,
    HelloDatasetMeta, HelloEntitySummary, HelloLexiconEntry,
};
use crate::sources::helloao::entries::{
    FetchedEntityDataset, fetch_entity_dataset, persist_entity_dataset,
};
use crate::sources::models::{
    GenericBook, GenericBookChapters, GenericCatalogBible, GenericChapter, GenericInstalledBible,
    TextDirection,
};
use crate::sources::{BibleSource, InstallPhase, InstallProgressSink};

const MAX_CONCURRENT_COMMENTARY_FETCHES: usize = 24;

/// Adapter exposing HelloAO data and installation operations through `BibleSource`
/// and source-specific helpers[cite: 7].
#[derive(Debug, Clone)]
pub struct HelloAOSource {
    client: HelloAOClient,
}

impl HelloAOSource {
    /// Creates a new `HelloAOSource` using the default HelloAO endpoint[cite: 7].
    pub fn new() -> Self {
        let client: HelloAOClient = HelloAOClient::new();
        Self { client }
    }

    /// Creates a new `HelloAOSource` with a custom base URL (useful for tests
    /// or alternative mirrors)[cite: 7].
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        let client: HelloAOClient = HelloAOClient::with_base_url(base_url);
        Self { client }
    }

    /// Returns a reference to the underlying `HelloAOClient` for source-specific
    /// operations (such as HelloAO commentaries, profiles, or datasets)[cite: 7].
    pub fn client(&self) -> &HelloAOClient {
        &self.client
    }

    // ── Translation & Entity Install / Uninstall ──────────────────────────────

    /// Downloads, normalizes, and persists a HelloAO translation into SQLite[cite: 7].
    pub fn install_translation(
        &self,
        conn: &Connection,
        translation_id: &str,
        progress: &dyn InstallProgressSink,
    ) -> Result<GenericInstalledBible, XBibleError> {
        let bundle: GenericInstalledBible = self
            .fetch_for_install(translation_id, progress)
            .map_err(|msg: String| XBibleError::Generic { error_message: msg })?;
        persist_installed_bible(conn, &bundle)?;
        Ok(bundle)
    }

    /// Fetches an entity dataset from HelloAO[cite: 3, 7].
    pub fn fetch_entities_for_install(
        &self,
        dataset_id: &str,
        progress: &dyn InstallProgressSink,
    ) -> Result<FetchedEntityDataset, String> {
        fetch_entity_dataset(&self.client, dataset_id, progress)
    }

    /// Fetches and persists an entity dataset (`people`, `events`, `places`,
    /// `people_groups`, and `entity_install_state`) into SQLite[cite: 3, 7].
    pub fn install_entity_dataset(
        &self,
        conn: &Connection,
        dataset_id: &str,
        progress: &dyn InstallProgressSink,
    ) -> Result<FetchedEntityDataset, XBibleError> {
        let dataset: FetchedEntityDataset =
            fetch_entity_dataset(&self.client, dataset_id, progress)
                .map_err(|msg: String| XBibleError::Generic { error_message: msg })?;

        let total_entities: usize = dataset.entities.len();
        progress.on_progress(
            InstallPhase::Install,
            0,
            total_entities as u32,
            Some(format!(
                "Persisting {total_entities} entities for {dataset_id}"
            )),
        );

        persist_entity_dataset(conn, &dataset, &mut |done, total, label| {
            progress.on_progress(
                InstallPhase::Install,
                done as u32,
                total as u32,
                Some(format!("Writing {label} ({done}/{total})")),
            );
        })?;

        progress.on_progress(
            InstallPhase::Done,
            1,
            1,
            Some(format!("{dataset_id} installed")),
        );

        Ok(dataset)
    }

    /// Removes installed rows for a HelloAO translation and clears its `installed` flag[cite: 4, 7].
    pub fn uninstall_translation(
        &self,
        conn: &Connection,
        translation_id: &str,
    ) -> Result<(), XBibleError> {
        uninstall_translation(conn, translation_id)
    }

    // ── Source-Specific Helpers (HelloAO Commentaries & Datasets) ────────────

    /// Lists available HelloAO commentaries[cite: 7].
    pub fn commentaries(&self) -> Result<Vec<HelloCommentaryMeta>, String> {
        let list: Vec<HelloCommentaryMeta> = self.client.commentaries()?;
        Ok(list)
    }

    /// Lists books for a HelloAO commentary normalized into `GenericBook`[cite: 7].
    pub fn commentary_books(&self, commentary_id: &str) -> Result<Vec<GenericBook>, String> {
        let books: Vec<GenericBook> = self.client.commentary_books(commentary_id)?;
        Ok(books)
    }

    /// Fetches a single HelloAO commentary chapter normalized into `GenericChapter`[cite: 7].
    pub fn commentary_chapter(
        &self,
        commentary_id: &str,
        book_id: &str,
        chapter: u32,
    ) -> Result<GenericChapter, String> {
        let ch: GenericChapter = self
            .client
            .commentary_chapter(commentary_id, book_id, chapter)?;
        Ok(ch)
    }

    /// Lists available profiles for a HelloAO commentary[cite: 7].
    pub fn commentary_profiles(
        &self,
        commentary_id: &str,
    ) -> Result<Vec<HelloCommentaryProfile>, String> {
        let profiles: Vec<HelloCommentaryProfile> =
            self.client.commentary_profiles(commentary_id)?;
        Ok(profiles)
    }

    /// Fetches a specific profile for a HelloAO commentary[cite: 7].
    pub fn commentary_profile(
        &self,
        commentary_id: &str,
        profile_id: &str,
    ) -> Result<HelloCommentaryProfile, String> {
        let profile: HelloCommentaryProfile =
            self.client.commentary_profile(commentary_id, profile_id)?;
        Ok(profile)
    }

    /// Lists available HelloAO datasets[cite: 7].
    pub fn datasets(&self) -> Result<Vec<HelloDatasetMeta>, String> {
        let list: Vec<HelloDatasetMeta> = self.client.datasets()?;
        Ok(list)
    }

    /// Lists people entities for a HelloAO dataset[cite: 7].
    pub fn dataset_people_list(&self, dataset_id: &str) -> Result<Vec<HelloEntitySummary>, String> {
        let list: Vec<HelloEntitySummary> = self.client.dataset_people_list(dataset_id)?;
        Ok(list)
    }

    /// Lists event entities for a HelloAO dataset[cite: 7].
    pub fn dataset_events_list(&self, dataset_id: &str) -> Result<Vec<HelloEntitySummary>, String> {
        let list: Vec<HelloEntitySummary> = self.client.dataset_events_list(dataset_id)?;
        Ok(list)
    }

    /// Lists place entities for a HelloAO dataset[cite: 7].
    pub fn dataset_places_list(&self, dataset_id: &str) -> Result<Vec<HelloEntitySummary>, String> {
        let list: Vec<HelloEntitySummary> = self.client.dataset_places_list(dataset_id)?;
        Ok(list)
    }

    /// Lists group entities for a HelloAO dataset[cite: 7].
    pub fn dataset_groups_list(&self, dataset_id: &str) -> Result<Vec<HelloEntitySummary>, String> {
        let list: Vec<HelloEntitySummary> = self.client.dataset_groups_list(dataset_id)?;
        Ok(list)
    }

    /// Fetches raw JSON for a single person entity in a HelloAO dataset[cite: 7].
    pub fn dataset_person(
        &self,
        dataset_id: &str,
        entity_id: &str,
    ) -> Result<serde_json::Value, String> {
        let val: serde_json::Value = self.client.dataset_person(dataset_id, entity_id)?;
        Ok(val)
    }

    /// Fetches raw JSON for a single event entity in a HelloAO dataset[cite: 7].
    pub fn dataset_event(
        &self,
        dataset_id: &str,
        entity_id: &str,
    ) -> Result<serde_json::Value, String> {
        let val: serde_json::Value = self.client.dataset_event(dataset_id, entity_id)?;
        Ok(val)
    }

    /// Fetches raw JSON for a single place entity in a HelloAO dataset[cite: 7].
    pub fn dataset_place(
        &self,
        dataset_id: &str,
        entity_id: &str,
    ) -> Result<serde_json::Value, String> {
        let val: serde_json::Value = self.client.dataset_place(dataset_id, entity_id)?;
        Ok(val)
    }

    /// Fetches raw JSON for a single group entity in a HelloAO dataset[cite: 7].
    pub fn dataset_group(
        &self,
        dataset_id: &str,
        entity_id: &str,
    ) -> Result<serde_json::Value, String> {
        let val: serde_json::Value = self.client.dataset_group(dataset_id, entity_id)?;
        Ok(val)
    }

    /// Fetches a lexicon entry by Strong's code from a HelloAO dataset[cite: 7].
    pub fn lexicon_entry(
        &self,
        dataset_id: &str,
        strongs: &str,
    ) -> Result<Option<HelloLexiconEntry>, String> {
        let entry: Option<HelloLexiconEntry> = self.client.lexicon_entry(dataset_id, strongs)?;
        Ok(entry)
    }
}

impl Default for HelloAOSource {
    fn default() -> Self {
        Self::new()
    }
}

impl BibleSource for HelloAOSource {
    fn source_id(&self) -> &str {
        HELLOAO_SOURCE_ID
    }

    fn display_name(&self) -> &str {
        "HelloAO Bible API"
    }

    fn list_catalog(&self) -> Result<Vec<GenericCatalogBible>, String> {
        self.client.list_catalog()
    }

    fn list_items(&self) -> Result<Vec<StoreItem>, String> {
        let mut items: Vec<StoreItem> = Vec::new();
        let sid: String = HELLOAO_SOURCE_ID.to_string();

        // 1. Translations -> StoreItem[cite: 7]
        let translations: Vec<GenericCatalogBible> = self.client.list_catalog()?;
        for t in translations {
            items.push(StoreItem {
                id: t.id.clone(),
                source_id: sid.clone(),
                kind: "translation".to_string(),
                name: t.name.clone(),
                english_name: t.english_name.clone().unwrap_or_else(|| t.name.clone()),
                short_name: t.short_name.clone().unwrap_or_else(|| t.id.clone()),
                language: t.language_code.clone(),
                language_name: t.language_name.clone(),
                description: format!(
                    "{} • {} books • {} verses",
                    t.language_english_name
                        .clone()
                        .unwrap_or_else(|| t.language_code.clone()),
                    t.book_count.unwrap_or(0),
                    t.verse_count.unwrap_or(0),
                ),
                badge: None,
                installed: false,
                has_strongs: t.has_strongs.unwrap_or(false),
                has_audio: t.has_audio,
                book_count: t.book_count.unwrap_or(0),
                chapter_count: t.chapter_count.unwrap_or(0),
                verse_count: t.verse_count.unwrap_or(0),
            });
        }

        // 2. Commentaries -> StoreItem[cite: 7]
        if let Ok(comms) = self.client.commentaries() {
            for c in comms {
                items.push(StoreItem {
                    id: c.id.clone(),
                    source_id: sid.clone(),
                    kind: "commentary".to_string(),
                    name: c.name.clone(),
                    english_name: c.english_name.clone(),
                    short_name: c.id.clone(),
                    language: c.language.clone(),
                    language_name: None,
                    description: format!(
                        "{} • {} books • {} chapters",
                        c.language, c.number_of_books, c.total_number_of_chapters
                    ),
                    badge: Some("Commentary".to_string()),
                    installed: false,
                    has_strongs: false,
                    has_audio: false,
                    book_count: c.number_of_books,
                    chapter_count: c.total_number_of_chapters,
                    verse_count: c.total_number_of_verses,
                });
            }
        }

        // 3. Datasets & Relationship Modules -> StoreItem[cite: 7]
        if let Ok(datasets) = self.client.datasets() {
            for d in datasets {
                let is_theographic: bool = d.id.to_ascii_lowercase().contains("theographic");
                let kind: &str = if is_theographic {
                    "entity_set"
                } else {
                    "dataset"
                };
                let badge: Option<String> = if is_theographic {
                    Some("Relationship Module".to_string())
                } else {
                    Some("Dataset".to_string())
                };

                items.push(StoreItem {
                    id: d.id.clone(),
                    source_id: sid.clone(),
                    kind: kind.to_string(),
                    name: d.name.clone(),
                    english_name: d.english_name.clone(),
                    short_name: d.id.clone(),
                    language: d.language.clone(),
                    language_name: None,
                    description: format!("{} dataset ({})", d.english_name, d.language),
                    badge,
                    installed: false,
                    has_strongs: false,
                    has_audio: false,
                    book_count: d.total_number_of_books,
                    chapter_count: 0,
                    verse_count: 0,
                });
            }
        }

        Ok(items)
    }

    fn list_books(&self, bible_id: &str) -> Result<Vec<GenericBook>, String> {
        self.client.list_books(bible_id)
    }

    fn fetch_chapter(
        &self,
        bible_id: &str,
        book_id: &str,
        chapter_number: i32,
    ) -> Result<GenericChapter, String> {
        self.client.fetch_chapter(bible_id, book_id, chapter_number)
    }

    fn fetch_for_install(
        &self,
        bible_id: &str,
        progress: &dyn InstallProgressSink,
    ) -> Result<GenericInstalledBible, String> {
        self.client.fetch_for_install(bible_id, progress)
    }

    fn install_item(
        &self,
        item: &StoreItem,
        progress: &dyn InstallProgressSink,
    ) -> Result<GenericInstalledBible, String> {
        if progress.is_cancelled() {
            return Err("Installation cancelled".to_string());
        }

        match item.kind.as_str() {
            "commentary" => {
                progress.on_progress(
                    InstallPhase::Catalog,
                    0,
                    2,
                    Some(format!("Fetching commentary books for {}", item.id)),
                );

                let completed_counter: Arc<AtomicU32> = Arc::new(AtomicU32::new(0));
                let counter_for_loop = Arc::clone(&completed_counter);
                let books: Vec<GenericBook> = self.client.commentary_books(&item.id)?;
                let mut tasks: Vec<(String, u32)> = Vec::new();
                for book in &books {
                    for ch_num in 1..=book.number_of_chapters {
                        if ch_num > 0 {
                            tasks.push((book.id.clone(), ch_num as u32));
                        }
                    }
                }

                let total_chapters: u32 = tasks.len() as u32;
                progress.on_progress(
                    InstallPhase::Download,
                    0,
                    total_chapters,
                    Some(format!(
                        "Downloading {} commentary chapters across {} books",
                        total_chapters,
                        books.len()
                    )),
                );

                let runtime = get_shared_runtime();
                let semaphore: Arc<Semaphore> =
                    Arc::new(Semaphore::new(MAX_CONCURRENT_COMMENTARY_FETCHES));
                let client_arc: Arc<HelloAOClient> = Arc::new(self.client.clone());
                let commentary_id_arc: Arc<String> = Arc::new(item.id.clone());

                let fetched: Vec<(String, GenericChapter)> = runtime.block_on(async move {
                    let mut join_set: JoinSet<Option<(String, GenericChapter)>> = JoinSet::new();

                    for (book_id, ch_num) in tasks {
                        if progress.is_cancelled() {
                            break;
                        }
                        let permit = match Arc::clone(&semaphore).acquire_owned().await {
                            Ok(p) => p,
                            Err(_) => break,
                        };
                        let client_ref = Arc::clone(&client_arc);
                        let cid_ref = Arc::clone(&commentary_id_arc);

                        join_set.spawn(async move {
                            let _permit = permit;
                            let book_id_for_fetch = book_id.clone();
                            let cid_for_fetch = (*cid_ref).clone();

                            let res: Result<GenericChapter, String> =
                                tokio::task::spawn_blocking(move || {
                                    client_ref.commentary_chapter(
                                        &cid_for_fetch,
                                        &book_id_for_fetch,
                                        ch_num,
                                    )
                                })
                                .await
                                .unwrap_or_else(|e: JoinError| Err(e.to_string()));

                            match res {
                                Ok(ch) => Some((book_id, ch)),
                                Err(err) => {
                                    log::warn!(
                                        "helloao commentary {} {} ch {} skipped: {}",
                                        *cid_ref,
                                        book_id,
                                        ch_num,
                                        err
                                    );
                                    None
                                }
                            }
                        });
                    }

                    let mut out: Vec<(String, GenericChapter)> = Vec::new();

                    while let Some(joined) = join_set.join_next().await {
                        if let Ok(Some(pair)) = joined {
                            out.push(pair);
                            let done: u32 = counter_for_loop.fetch_add(1, Ordering::Relaxed) + 1;
                            progress.on_progress(
                                InstallPhase::Download,
                                done,
                                total_chapters,
                                Some(format!("Downloaded {}/{} chapters", done, total_chapters)),
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
                    return Err("Commentary installation cancelled".to_string());
                }

                progress.on_progress(
                    InstallPhase::Normalize,
                    1,
                    2,
                    Some("Structuring commentary books and chapters".to_string()),
                );

                let mut grouped: HashMap<String, Vec<GenericChapter>> = HashMap::new();
                for (book_id, ch) in fetched {
                    grouped.entry(book_id).or_default().push(ch);
                }

                let mut chapters_by_book: Vec<GenericBookChapters> =
                    Vec::with_capacity(books.len());
                for book in &books {
                    let mut chs: Vec<GenericChapter> = grouped.remove(&book.id).unwrap_or_default();
                    chs.sort_by_key(|c: &GenericChapter| c.chapter_number);
                    chapters_by_book.push(GenericBookChapters {
                        book_id: book.id.clone(),
                        chapters: chs,
                    });
                }

                progress.on_progress(InstallPhase::Done, 2, 2, None);

                Ok(GenericInstalledBible {
                    catalog: GenericCatalogBible {
                        id: item.id.clone(),
                        source_id: HELLOAO_SOURCE_ID.to_string(),
                        name: item.name.clone(),
                        english_name: Some(item.english_name.clone()),
                        short_name: Some(item.short_name.clone()),
                        language_code: item.language.clone(),
                        language_name: item.language_name.clone(),
                        language_english_name: None,
                        text_direction: TextDirection::LeftToRight,
                        book_count: Some(books.len() as i32),
                        chapter_count: Some(total_chapters as i32),
                        verse_count: Some(0),
                        has_text: true,
                        has_audio: false,
                        has_strongs: Some(false),
                        license_url: None,
                        license_notes: None,
                        audio_sources: Vec::new(),
                    },
                    books,
                    chapters_by_book,
                })
            }

            "entity_set" | "dataset" => {
                let dataset: FetchedEntityDataset =
                    fetch_entity_dataset(&self.client, &item.id, progress)?;

                let total_entities: usize = dataset.entities.len();
                progress.on_progress(
                    InstallPhase::Install,
                    0,
                    total_entities as u32,
                    Some(format!(
                        "Prepared {total_entities} entities for {}",
                        item.id
                    )),
                );

                progress.on_progress(
                    InstallPhase::Done,
                    1,
                    1,
                    Some(format!("{} fetched ({total_entities} entities)", item.id)),
                );

                Ok(GenericInstalledBible {
                    catalog: GenericCatalogBible {
                        id: item.id.clone(),
                        source_id: HELLOAO_SOURCE_ID.to_string(),
                        name: item.name.clone(),
                        english_name: Some(item.english_name.clone()),
                        short_name: Some(item.short_name.clone()),
                        language_code: item.language.clone(),
                        language_name: item.language_name.clone(),
                        language_english_name: None,
                        text_direction: TextDirection::LeftToRight,
                        book_count: Some(0),
                        chapter_count: Some(0),
                        verse_count: Some(0),
                        has_text: false,
                        has_audio: false,
                        has_strongs: Some(false),
                        license_url: None,
                        license_notes: None,
                        audio_sources: Vec::new(),
                    },
                    books: Vec::new(),
                    chapters_by_book: Vec::new(),
                })
            }
            _ => self.client.fetch_for_install(&item.id, progress),
        }
    }
}
