//! HelloAO HTTP client and raw-JSON-to-generic normalizer (`sources/helloao/client.rs`).
//!
//! All HelloAO-specific wire types, endpoint paths, and JSON parsing quirks
//! (`books.json` `isApocryphal` fallback, `complete.json` streaming download,
//! bounded concurrent chapter fetching over the shared Tokio runtime, dataset
//! count fallbacks, and flexible deserializers) live here and map into the
//! source-agnostic `Generic*` structs consumed by the Store and DB[cite: 6].

use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use reqwest::blocking::{Client, Response};
use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Deserializer, Serialize};
use tokio::runtime::Runtime;
use tokio::sync::Semaphore;
use tokio::task::{JoinError, JoinSet};

use crate::runtime::get_shared_runtime;
use crate::sources::InstallProgressSink;
use crate::sources::models::{
    CanonicalSection, GenericAudioSource, GenericAudioTiming, GenericBook, GenericBookChapters,
    GenericCatalogBible, GenericChapter, GenericChapterItem, GenericFootnote,
    GenericInstalledBible, GenericTextSpan, GenericVerse, LexicalInfo, SpanFormatting, SpanKind,
    TextDirection,
};

pub const HELLOAO_SOURCE_ID: &str = "helloao";
pub const HELLOAO_DEFAULT_BASE_URL: &str = "https://bible.helloao.org/api";
const MAX_CONCURRENT_CHAPTER_FETCHES: usize = 8;

// ═════════════════════════════════════════════════════════════════════════════
// HelloAO-Specific Data Views (Internal / Source-Local)
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloCommentaryMeta {
    pub id: String,
    pub name: String,
    pub english_name: String,
    pub language: String,
    pub license_url: Option<String>,
    pub license_notes: Option<String>,
    pub number_of_books: i32,
    pub total_number_of_chapters: i32,
    pub total_number_of_verses: i32,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct HelloCommentaryProfile {
    pub id: String,
    pub name: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloDatasetMeta {
    pub id: String,
    pub name: String,
    pub english_name: String,
    pub language: String,
    pub license_url: Option<String>,
    pub license_notes: Option<String>,
    pub total_number_of_books: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloLexiconEntry {
    pub strongs: String,
    pub lemma: Option<String>,
    pub transliteration: Option<String>,
    pub part_of_speech: Option<String>,
    pub gloss: Option<String>,
    pub definition: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloEntitySummary {
    pub id: String,
    pub name: String,
    pub is_proper_name: Option<bool>,
    pub gender: Option<String>,
    pub start_date: Option<String>,
    pub number_of_references: i32,
}

#[derive(Debug, Clone)]
struct FetchedChapterPair {
    book_id: String,
    chapter: GenericChapter,
}

// ═════════════════════════════════════════════════════════════════════════════
// HelloAO Client Implementation
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct HelloAOClient {
    http: Client,
    base_url: String,
}

impl Default for HelloAOClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HelloAOClient {
    pub fn new() -> Self {
        Self::with_base_url(HELLOAO_DEFAULT_BASE_URL)
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        #[cfg(target_os = "android")]
        let http: Client = {
            use rustls::RootCertStore;

            let mut roots: RootCertStore = RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config: rustls::ClientConfig = rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth();

            match Client::builder()
                .http1_only()
                .pool_max_idle_per_host(32)
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(600))
                .tcp_keepalive(Duration::from_secs(15))
                .use_preconfigured_tls(config)
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    log::warn!(
                        "HelloAOClient: android preconfigured client build failed ({}), \
                         falling back to reqwest::blocking::Client::new()",
                        e
                    );
                    Client::new()
                }
            }
        };

        #[cfg(not(target_os = "android"))]
        let http: Client = match Client::builder()
            .http1_only()
            .pool_max_idle_per_host(32)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(600))
            .tcp_keepalive(Duration::from_secs(15))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                log::warn!(
                    "HelloAOClient: client build failed ({}), \
                     falling back to reqwest::blocking::Client::new()",
                    e
                );
                Client::new()
            }
        };

        Self {
            http,
            base_url: base_url.into(),
        }
    }

    // ── Catalog & Structure -> Generic* ──────────────────────────────────────

    /// Fetches available HelloAO translations normalized into `GenericCatalogBible`.
    pub fn list_catalog(&self) -> Result<Vec<GenericCatalogBible>, String> {
        let data: TranslationsResponse = self.get_json("available_translations.json")?;
        let items: Vec<GenericCatalogBible> = data
            .translations
            .into_iter()
            .map(|t: RawTranslation| -> GenericCatalogBible {
                let text_direction: TextDirection =
                    parse_text_direction(t.text_direction.as_deref());
                let audio_sources: Vec<GenericAudioSource> =
                    parse_translation_audio_sources(&t.raw);
                let has_audio: bool = !audio_sources.is_empty();

                GenericCatalogBible {
                    id: t.id,
                    source_id: HELLOAO_SOURCE_ID.to_string(),
                    name: t.name,
                    english_name: Some(t.english_name),
                    short_name: Some(t.short_name),
                    language_code: t.language,
                    language_name: t.language_name,
                    language_english_name: t.language_english_name,
                    text_direction,
                    book_count: Some(t.number_of_books),
                    chapter_count: Some(t.total_number_of_chapters),
                    verse_count: Some(t.total_number_of_verses),
                    has_text: true,
                    has_audio,
                    has_strongs: None,
                    license_url: t.license_url,
                    license_notes: t.license_notes,
                    audio_sources,
                }
            })
            .collect();
        Ok(items)
    }

    /// Fetches books for a HelloAO translation normalized into `GenericBook`.
    pub fn list_books(&self, translation_id: &str) -> Result<Vec<GenericBook>, String> {
        let data: BooksResponse = self.get_json(&format!("{}/books.json", translation_id))?;
        let books: Vec<GenericBook> = data
            .books
            .into_iter()
            .map(|b: RawBook| -> GenericBook {
                let is_apocryphal: bool = b.is_apocryphal.unwrap_or(false);
                let section: CanonicalSection = infer_section(&b.id, is_apocryphal);
                let audio_sources: Vec<GenericAudioSource> =
                    parse_audio_sources(b.audio_links.as_ref());
                let audio_links_json: Option<String> = serialize_opt_json(b.audio_links.as_ref());
                let raw_json: Option<String> = serialize_opt_json(Some(&b));

                GenericBook {
                    translation_id: translation_id.to_string(),
                    source_id: HELLOAO_SOURCE_ID.to_string(),
                    id: b.id,
                    name: b.name,
                    common_name: b.common_name,
                    title: b.title,
                    book_order: b.order,
                    section,
                    number_of_chapters: b.number_of_chapters,
                    audio_sources,
                    audio_links_json,
                    raw_json,
                }
            })
            .collect();
        Ok(books)
    }

    /// Fetches a single chapter and normalizes it into `GenericChapter`.
    pub fn fetch_chapter(
        &self,
        translation_id: &str,
        book_id: &str,
        chapter_number: i32,
    ) -> Result<GenericChapter, String> {
        let data: ChapterResponse = self.get_json(&format!(
            "{}/{}/{}.json",
            translation_id, book_id, chapter_number
        ))?;
        let raw_json: String = serialize_json(&data.chapter);

        let (items, verses, footnotes): (
            Vec<GenericChapterItem>,
            Vec<GenericVerse>,
            Vec<GenericFootnote>,
        ) = build_chapter_content(book_id, chapter_number, data.chapter.content, Vec::new());

        Ok(GenericChapter {
            translation_id: translation_id.to_string(),
            source_id: HELLOAO_SOURCE_ID.to_string(),
            book_id: book_id.to_string(),
            chapter_number,
            items,
            verses,
            footnotes,
            audio_sources: Vec::new(),
            audio_timings: Vec::new(),
            raw_json,
        })
    }

    /// Fetches a simple verse-only chapter and normalizes it into `GenericChapter`.
    pub fn fetch_chapter_simple(
        &self,
        translation_id: &str,
        book_id: &str,
        chapter_number: i32,
    ) -> Result<GenericChapter, String> {
        let data: SimpleChapterResponse = self.get_json(&format!(
            "{}/{}/{}.simple.json",
            translation_id, book_id, chapter_number
        ))?;
        let chapter_raw_json: String = serialize_json(&data.chapter);

        let mut verses: Vec<GenericVerse> = Vec::new();
        let mut items: Vec<GenericChapterItem> = Vec::new();

        for (idx, item) in data.chapter.content.into_iter().enumerate() {
            if let FlexibleSimpleItem::Verse {
                number: Some(num),
                text,
            } = item
            {
                let item_raw_json: Option<String> = serialize_opt_json(Some(&serde_json::json!({
                    "type": "verse",
                    "number": num,
                    "text": text,
                })));
                verses.push(GenericVerse {
                    book_id: book_id.to_string(),
                    chapter: chapter_number,
                    verse: num,
                    text: text.clone(),
                });
                items.push(GenericChapterItem {
                    position: idx as i32,
                    item_type: "verse".to_string(),
                    verse_number: Some(num),
                    text: Some(text.clone()),
                    is_poem: false,
                    is_words_of_jesus: false,
                    footnote_ids: Vec::new(),
                    spans: vec![GenericTextSpan {
                        position: 0,
                        kind: SpanKind::Text,
                        text,
                        formatting: SpanFormatting::default(),
                        lexical: None,
                        footnote_ids: Vec::new(),
                        cross_ref_ids: Vec::new(),
                        language_code: None,
                        raw_json: item_raw_json.clone(),
                    }],
                    raw_json: item_raw_json,
                });
            }
        }

        Ok(GenericChapter {
            translation_id: translation_id.to_string(),
            source_id: HELLOAO_SOURCE_ID.to_string(),
            book_id: book_id.to_string(),
            chapter_number,
            items,
            verses,
            footnotes: Vec::new(),
            audio_sources: Vec::new(),
            audio_timings: Vec::new(),
            raw_json: chapter_raw_json,
        })
    }

    /// Downloads and normalizes a complete HelloAO translation into `GenericInstalledBible`,
    /// falling back to bounded concurrent per-chapter fetches on the shared Tokio
    /// runtime when `complete.json` is unavailable.
    pub fn fetch_for_install(
        &self,
        translation_id: &str,
        progress: &dyn InstallProgressSink,
    ) -> Result<GenericInstalledBible, String> {
        if progress.is_cancelled() {
            return Err("Installation cancelled before start".to_string());
        }

        progress.on_progress(
            "catalog".to_string(),
            0,
            3,
            Some(format!("Fetching metadata for {}", translation_id)),
        );

        let catalog_entry: GenericCatalogBible = self
            .list_catalog()?
            .into_iter()
            .find(|t: &GenericCatalogBible| t.id == translation_id)
            .unwrap_or_else(|| GenericCatalogBible {
                id: translation_id.to_string(),
                source_id: HELLOAO_SOURCE_ID.to_string(),
                name: translation_id.to_string(),
                english_name: None,
                short_name: None,
                language_code: "und".to_string(),
                language_name: None,
                language_english_name: None,
                text_direction: TextDirection::LeftToRight,
                book_count: None,
                chapter_count: None,
                verse_count: None,
                has_text: true,
                has_audio: false,
                has_strongs: None,
                license_url: None,
                license_notes: None,
                audio_sources: Vec::new(),
            });

        if progress.is_cancelled() {
            return Err("Installation cancelled".to_string());
        }

        // `books.json` is the authoritative source for `isApocryphal` across books.
        let books_list: Vec<GenericBook> = self.list_books(translation_id)?;
        let apocryphal_by_id: HashMap<String, bool> = books_list
            .iter()
            .map(|b: &GenericBook| {
                (
                    b.id.clone(),
                    matches!(b.section, CanonicalSection::Deuterocanon),
                )
            })
            .collect();

        progress.on_progress(
            "download".to_string(),
            1,
            3,
            Some("Downloading complete translation archive".to_string()),
        );

        let url: String = format!("{}/{}/complete.json", self.base_url, translation_id);
        let resp_res: Result<Response, reqwest::Error> = self.http.get(&url).send();

        if let Ok(mut resp) = resp_res {
            if resp.status().is_success() {
                let total: Option<u64> = resp.content_length();
                let total_u32: u32 = total.and_then(|v: u64| u32::try_from(v).ok()).unwrap_or(0);
                let mut buf: Vec<u8> =
                    Vec::with_capacity(total.unwrap_or(8 * 1024 * 1024) as usize);
                let mut chunk: [u8; 65536] = [0u8; 64 * 1024];
                let mut received: u64 = 0;

                loop {
                    if progress.is_cancelled() {
                        return Err("Installation cancelled during download".to_string());
                    }
                    let n: usize = resp
                        .read(&mut chunk)
                        .map_err(|e: std::io::Error| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    received += n as u64;
                    let current_u32: u32 = u32::try_from(received).unwrap_or(u32::MAX);
                    progress.on_progress("download".to_string(), current_u32, total_u32, None);
                }

                if progress.is_cancelled() {
                    return Err("Installation cancelled before normalization".to_string());
                }

                progress.on_progress(
                    "normalize".to_string(),
                    2,
                    3,
                    Some("Structuring books, chapters, and verses".to_string()),
                );

                let data: CompleteResponse =
                    serde_json::from_slice(&buf).map_err(|e: serde_json::Error| e.to_string())?;
                let normalized_bundle: GenericInstalledBible = normalize_complete_books(
                    translation_id,
                    catalog_entry,
                    data.books,
                    &apocryphal_by_id,
                );

                progress.on_progress("done".to_string(), 3, 3, None);
                return Ok(normalized_bundle);
            }
        }

        // Fallback: concurrent per-chapter fetch on the shared Tokio runtime.
        self.fetch_for_install_concurrent(translation_id, catalog_entry, books_list, progress)
    }

    /// Fetches all chapters across `books` concurrently with bounded parallelism
    /// (`MAX_CONCURRENT_CHAPTER_FETCHES`) on the shared Tokio runtime.
    pub fn fetch_for_install_concurrent(
        &self,
        translation_id: &str,
        catalog_entry: GenericCatalogBible,
        books: Vec<GenericBook>,
        progress: &dyn InstallProgressSink,
    ) -> Result<GenericInstalledBible, String> {
        let mut total_chapters: u32 = 0;
        let mut tasks: Vec<(GenericBook, i32)> = Vec::new();
        for book in &books {
            for ch_num in 1..=book.number_of_chapters {
                tasks.push((book.clone(), ch_num));
                total_chapters = total_chapters.saturating_add(1);
            }
        }

        progress.on_progress(
            "download".to_string(),
            0,
            total_chapters,
            Some(format!(
                "Fetching {} chapters across {} books",
                total_chapters,
                books.len()
            )),
        );

        let runtime: &'static Runtime = get_shared_runtime();
        let semaphore: Arc<Semaphore> = Arc::new(Semaphore::new(MAX_CONCURRENT_CHAPTER_FETCHES));
        let client_arc: Arc<HelloAOClient> = Arc::new(self.clone());
        let tid_arc: Arc<String> = Arc::new(translation_id.to_string());

        let fetched_pairs: Vec<FetchedChapterPair> = runtime.block_on(async move {
            let mut join_set: JoinSet<Option<FetchedChapterPair>> = JoinSet::new();

            for (book, ch_num) in tasks {
                let permit: tokio::sync::OwnedSemaphorePermit =
                    match Arc::clone(&semaphore).acquire_owned().await {
                        Ok(p) => p,
                        Err(_) => break,
                    };
                let client_ref: Arc<HelloAOClient> = Arc::clone(&client_arc);
                let tid_ref: Arc<String> = Arc::clone(&tid_arc);

                join_set.spawn(async move {
                    let _permit: tokio::sync::OwnedSemaphorePermit = permit;
                    let book_id: String = book.id.clone();
                    let tid_for_fetch: String = (*tid_ref).clone();
                    let tid_for_log: String = (*tid_ref).clone();
                    let book_id_for_log: String = book_id.clone();

                    let res: Result<GenericChapter, String> =
                        tokio::task::spawn_blocking(move || -> Result<GenericChapter, String> {
                            client_ref.fetch_chapter(&tid_for_fetch, &book_id, ch_num)
                        })
                        .await
                        .unwrap_or_else(|e: JoinError| Err(e.to_string()));

                    match res {
                        Ok(chapter) => Some(FetchedChapterPair {
                            book_id: book.id,
                            chapter,
                        }),
                        Err(err) => {
                            log::warn!(
                                "helloao {} {} ch {} skipped: {}",
                                tid_for_log,
                                book_id_for_log,
                                ch_num,
                                err
                            );
                            None
                        }
                    }
                });
            }

            let mut results: Vec<FetchedChapterPair> = Vec::new();
            let mut done: u32 = 0;

            while let Some(joined) = join_set.join_next().await {
                if let Ok(Some(pair)) = joined {
                    results.push(pair);
                    done = done.saturating_add(1);
                    progress.on_progress("download".to_string(), done, total_chapters, None);
                }
                if progress.is_cancelled() {
                    join_set.abort_all();
                    break;
                }
            }

            results
        });

        if progress.is_cancelled() {
            return Err("Installation cancelled".to_string());
        }

        progress.on_progress(
            "normalize".to_string(),
            2,
            3,
            Some("Structuring books, chapters, and verses".to_string()),
        );

        let mut grouped: HashMap<String, Vec<GenericChapter>> = HashMap::new();
        for pair in fetched_pairs {
            grouped.entry(pair.book_id).or_default().push(pair.chapter);
        }

        let mut chapters_by_book: Vec<GenericBookChapters> = Vec::with_capacity(books.len());
        for book in &books {
            let mut chs: Vec<GenericChapter> = grouped.remove(&book.id).unwrap_or_default();
            chs.sort_by_key(|c: &GenericChapter| c.chapter_number);
            chapters_by_book.push(GenericBookChapters {
                book_id: book.id.clone(),
                chapters: chs,
            });
        }

        progress.on_progress("done".to_string(), 3, 3, None);

        Ok(GenericInstalledBible {
            catalog: catalog_entry,
            books,
            chapters_by_book,
        })
    }

    // ── HelloAO Commentaries & Datasets (Source-Local) ────────────────────────

    pub fn commentaries(&self) -> Result<Vec<HelloCommentaryMeta>, String> {
        let data: CommentariesResponse = self.get_json("available_commentaries.json")?;
        let list: Vec<HelloCommentaryMeta> = data
            .commentaries
            .into_iter()
            .map(|c: RawCommentary| -> HelloCommentaryMeta {
                HelloCommentaryMeta {
                    id: c.id,
                    name: c.name,
                    english_name: c.english_name,
                    language: c.language,
                    license_url: c.license_url,
                    license_notes: c.license_notes,
                    number_of_books: c.number_of_books,
                    total_number_of_chapters: c.total_number_of_chapters,
                    total_number_of_verses: c.total_number_of_verses,
                }
            })
            .collect();
        Ok(list)
    }

    pub fn commentary_books(&self, commentary_id: &str) -> Result<Vec<GenericBook>, String> {
        let data: BooksResponse = self.get_json(&format!("c/{}/books.json", commentary_id))?;
        let books: Vec<GenericBook> = data
            .books
            .into_iter()
            .map(|b: RawBook| -> GenericBook {
                let is_apocryphal: bool = b.is_apocryphal.unwrap_or(false);
                let audio_links_json: Option<String> = serialize_opt_json(b.audio_links.as_ref());
                let raw_json: Option<String> = serialize_opt_json(Some(&b));

                GenericBook {
                    translation_id: commentary_id.to_string(),
                    source_id: HELLOAO_SOURCE_ID.to_string(),
                    id: b.id.clone(),
                    name: b.name,
                    common_name: b.common_name,
                    title: b.title,
                    book_order: b.order,
                    section: infer_section(&b.id, is_apocryphal),
                    number_of_chapters: b.number_of_chapters,
                    audio_sources: Vec::new(),
                    audio_links_json,
                    raw_json,
                }
            })
            .collect();
        Ok(books)
    }

    pub fn commentary_chapter(
        &self,
        commentary_id: &str,
        book_id: &str,
        chapter_number: u32,
    ) -> Result<GenericChapter, String> {
        let data: ChapterResponse = self.get_json(&format!(
            "c/{}/{}/{}.json",
            commentary_id, book_id, chapter_number
        ))?;
        let ch_num: i32 = chapter_number as i32;
        let raw_json: String = serialize_json(&data.chapter);
        let (items, verses, footnotes): (
            Vec<GenericChapterItem>,
            Vec<GenericVerse>,
            Vec<GenericFootnote>,
        ) = build_chapter_content(book_id, ch_num, data.chapter.content, Vec::new());

        Ok(GenericChapter {
            translation_id: commentary_id.to_string(),
            source_id: HELLOAO_SOURCE_ID.to_string(),
            book_id: book_id.to_string(),
            chapter_number: ch_num,
            items,
            verses,
            footnotes,
            audio_sources: Vec::new(),
            audio_timings: Vec::new(),
            raw_json,
        })
    }

    pub fn commentary_profiles(
        &self,
        commentary_id: &str,
    ) -> Result<Vec<HelloCommentaryProfile>, String> {
        let data: ProfilesResponse =
            self.get_json(&format!("c/{}/profiles.json", commentary_id))?;
        Ok(data.profiles)
    }

    pub fn commentary_profile(
        &self,
        commentary_id: &str,
        profile_id: &str,
    ) -> Result<HelloCommentaryProfile, String> {
        self.get_json(&format!("c/{}/profiles/{}.json", commentary_id, profile_id))
    }

    pub fn datasets(&self) -> Result<Vec<HelloDatasetMeta>, String> {
        let data: DatasetsResponse = self.get_json("available_datasets.json")?;
        let list: Vec<HelloDatasetMeta> = data
            .datasets
            .into_iter()
            .map(|d: RawDataset| -> HelloDatasetMeta {
                HelloDatasetMeta {
                    id: d.id,
                    name: d.name.clone(),
                    english_name: d.english_name.clone().unwrap_or(d.name),
                    language: d.language.clone().unwrap_or_else(|| "eng".to_string()),
                    license_url: d.license_url,
                    license_notes: d.license_notes,
                    total_number_of_books: d
                        .total_number_of_books
                        .or(d.total_number_of_words)
                        .or(d.total_number_of_lexicon)
                        .unwrap_or(0),
                }
            })
            .collect();
        Ok(list)
    }

    pub fn dataset_people_list(&self, dataset_id: &str) -> Result<Vec<HelloEntitySummary>, String> {
        let data: PeopleListResponse = self.get_json(&format!("d/{}/people.json", dataset_id))?;
        Ok(data
            .people
            .into_iter()
            .map(HelloEntitySummary::from)
            .collect())
    }

    pub fn dataset_events_list(&self, dataset_id: &str) -> Result<Vec<HelloEntitySummary>, String> {
        let data: EventsListResponse = self.get_json(&format!("d/{}/events.json", dataset_id))?;
        Ok(data
            .events
            .into_iter()
            .map(HelloEntitySummary::from)
            .collect())
    }

    pub fn dataset_places_list(&self, dataset_id: &str) -> Result<Vec<HelloEntitySummary>, String> {
        let data: PlacesListResponse = self.get_json(&format!("d/{}/places.json", dataset_id))?;
        Ok(data
            .places
            .into_iter()
            .map(HelloEntitySummary::from)
            .collect())
    }

    pub fn dataset_groups_list(&self, dataset_id: &str) -> Result<Vec<HelloEntitySummary>, String> {
        let data: GroupsListResponse = self.get_json(&format!("d/{}/groups.json", dataset_id))?;
        Ok(data
            .groups
            .into_iter()
            .map(HelloEntitySummary::from)
            .collect())
    }

    pub fn dataset_person(
        &self,
        dataset_id: &str,
        entity_id: &str,
    ) -> Result<serde_json::Value, String> {
        self.get_json(&format!("d/{}/people/{}.json", dataset_id, entity_id))
    }

    pub fn dataset_event(
        &self,
        dataset_id: &str,
        entity_id: &str,
    ) -> Result<serde_json::Value, String> {
        self.get_json(&format!("d/{}/events/{}.json", dataset_id, entity_id))
    }

    pub fn dataset_place(
        &self,
        dataset_id: &str,
        entity_id: &str,
    ) -> Result<serde_json::Value, String> {
        self.get_json(&format!("d/{}/places/{}.json", dataset_id, entity_id))
    }

    pub fn dataset_group(
        &self,
        dataset_id: &str,
        entity_id: &str,
    ) -> Result<serde_json::Value, String> {
        self.get_json(&format!("d/{}/groups/{}.json", dataset_id, entity_id))
    }

    pub fn lexicon_entry(
        &self,
        dataset_id: &str,
        strongs: &str,
    ) -> Result<Option<HelloLexiconEntry>, String> {
        let url: String = format!(
            "{}/d/{}/lexicon/{}.json",
            self.base_url, dataset_id, strongs
        );
        let resp: Response = self.http.get(&url).send().map_err(|e| e.to_string())?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let data: LexiconEntryResponse = resp.json().map_err(|e| e.to_string())?;
        let sense: Option<RawLexiconSense> = data.entry.senses.into_iter().next();
        Ok(Some(HelloLexiconEntry {
            strongs: data.entry.id,
            lemma: sense
                .as_ref()
                .and_then(|s: &RawLexiconSense| s.lemma.clone()),
            transliteration: sense
                .as_ref()
                .and_then(|s: &RawLexiconSense| s.transliteration.clone()),
            part_of_speech: sense
                .as_ref()
                .and_then(|s: &RawLexiconSense| s.part_of_speech.clone()),
            gloss: sense
                .as_ref()
                .and_then(|s: &RawLexiconSense| s.gloss.clone()),
            definition: sense
                .as_ref()
                .and_then(|s: &RawLexiconSense| s.definition.clone()),
        }))
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let url: String = format!("{}/{}", self.base_url, path);
        let resp: Response = self.http.get(&url).send().map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {} for {}", resp.status(), url));
        }
        resp.json::<T>().map_err(|e| e.to_string())
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Normalization Helpers (Raw HelloAO -> Generic*)
// ═════════════════════════════════════════════════════════════════════════════

fn serialize_json<T: Serialize>(val: &T) -> String {
    serde_json::to_string(val).unwrap_or_default()
}

fn serialize_opt_json<T: Serialize>(val: Option<&T>) -> Option<String> {
    val.and_then(|v| serde_json::to_string(v).ok())
}

fn normalize_complete_books(
    translation_id: &str,
    catalog: GenericCatalogBible,
    raw_books: Vec<RawCompleteBook>,
    apocryphal_by_id: &HashMap<String, bool>,
) -> GenericInstalledBible {
    let mut books: Vec<GenericBook> = Vec::with_capacity(raw_books.len());
    let mut chapters_by_book: Vec<GenericBookChapters> = Vec::with_capacity(raw_books.len());

    for b in raw_books {
        let is_apocryphal: bool = apocryphal_by_id
            .get(&b.id)
            .copied()
            .or(b.is_apocryphal)
            .unwrap_or(false);
        let section: CanonicalSection = infer_section(&b.id, is_apocryphal);
        let book_audio: Vec<GenericAudioSource> = parse_audio_sources(b.audio_links.as_ref());
        let audio_links_json: Option<String> = serialize_opt_json(b.audio_links.as_ref());
        let book_raw_json: Option<String> = serialize_opt_json(Some(&b));

        books.push(GenericBook {
            translation_id: translation_id.to_string(),
            source_id: HELLOAO_SOURCE_ID.to_string(),
            id: b.id.clone(),
            name: b.name,
            common_name: b.common_name,
            title: b.title,
            book_order: b.order,
            section,
            number_of_chapters: b.number_of_chapters,
            audio_sources: book_audio.clone(),
            audio_links_json,
            raw_json: book_raw_json,
        });

        let mut book_chapters: Vec<GenericChapter> = Vec::with_capacity(b.chapters.len());
        for wrap in b.chapters {
            let c: RawChapterPayload = wrap.chapter;
            let chapter_raw_json: String = serialize_json(&c);
            let footnotes: Vec<GenericFootnote> = c
                .footnotes
                .into_iter()
                .map(|f: RawFootnote| -> GenericFootnote {
                    let note_raw_json: Option<String> = serialize_opt_json(Some(&f));
                    GenericFootnote {
                        note_id: f.note_id,
                        caller: f.caller,
                        text: Some(f.text),
                        reference: f.reference,
                        raw_json: note_raw_json,
                    }
                })
                .collect();

            let audio_timings: Vec<GenericAudioTiming> = c
                .audio_timings
                .into_iter()
                .map(|t: RawAudioTiming| -> GenericAudioTiming {
                    GenericAudioTiming {
                        verse: t.verse.unwrap_or(0),
                        start_ms: t.start_ms,
                        end_ms: t.end_ms,
                    }
                })
                .collect();

            let (items, verses, chapter_footnotes): (
                Vec<GenericChapterItem>,
                Vec<GenericVerse>,
                Vec<GenericFootnote>,
            ) = build_chapter_content(&b.id, c.number, c.content, footnotes);

            book_chapters.push(GenericChapter {
                translation_id: translation_id.to_string(),
                source_id: HELLOAO_SOURCE_ID.to_string(),
                book_id: b.id.clone(),
                chapter_number: c.number,
                items,
                verses,
                footnotes: chapter_footnotes,
                audio_sources: book_audio.clone(),
                audio_timings,
                raw_json: chapter_raw_json,
            });
        }

        chapters_by_book.push(GenericBookChapters {
            book_id: b.id,
            chapters: book_chapters,
        });
    }

    GenericInstalledBible {
        catalog,
        books,
        chapters_by_book,
    }
}

fn parse_text_direction(dir: Option<&str>) -> TextDirection {
    match dir.map(|s: &str| s.to_ascii_lowercase()).as_deref() {
        Some("rtl") | Some("right-to-left") | Some("right_to_left") => TextDirection::RightToLeft,
        _ => TextDirection::LeftToRight,
    }
}

fn parse_translation_audio_sources(raw: &serde_json::Value) -> Vec<GenericAudioSource> {
    raw.get("availableFormats")
        .and_then(|v: &serde_json::Value| v.as_array())
        .map(|arr: &Vec<serde_json::Value>| -> Vec<GenericAudioSource> {
            arr.iter()
                .filter_map(|v: &serde_json::Value| v.as_str())
                .filter(|s: &&str| {
                    let lower: String = s.to_ascii_lowercase();
                    lower.contains("audio") || lower.ends_with("mp3") || lower.ends_with("opus")
                })
                .map(|fmt: &str| -> GenericAudioSource {
                    GenericAudioSource {
                        source_ref_id: fmt.to_string(),
                        url: None,
                        format: Some(fmt.to_string()),
                        is_dramatized: None,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_audio_sources(audio_links: Option<&serde_json::Value>) -> Vec<GenericAudioSource> {
    let Some(val) = audio_links else {
        return Vec::new();
    };
    match val {
        serde_json::Value::Object(map) => map
            .iter()
            .filter_map(|(k, v): (&String, &serde_json::Value)| {
                v.as_str().map(|url: &str| GenericAudioSource {
                    source_ref_id: k.clone(),
                    url: Some(url.to_string()),
                    format: None,
                    is_dramatized: None,
                })
            })
            .collect(),
        serde_json::Value::Array(arr) => arr
            .iter()
            .enumerate()
            .filter_map(|(idx, v): (usize, &serde_json::Value)| {
                v.as_str().map(|url: &str| GenericAudioSource {
                    source_ref_id: format!("track_{}", idx),
                    url: Some(url.to_string()),
                    format: None,
                    is_dramatized: None,
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn infer_section(book_id: &str, is_apocryphal: bool) -> CanonicalSection {
    if is_apocryphal {
        return CanonicalSection::Deuterocanon;
    }
    match book_id.to_ascii_uppercase().as_str() {
        "GEN" | "EXO" | "LEV" | "NUM" | "DEU" | "JOS" | "JDG" | "RUT" | "1SA" | "2SA" | "1KI"
        | "2KI" | "1CH" | "2CH" | "EZR" | "NEH" | "EST" | "JOB" | "PSA" | "PRO" | "ECC" | "SNG"
        | "ISA" | "JER" | "LAM" | "EZK" | "DAN" | "HOS" | "JOL" | "AMO" | "OBA" | "JON" | "MIC"
        | "NAM" | "HAB" | "ZEP" | "HAG" | "ZEC" | "MAL" => CanonicalSection::OldTestament,
        "MAT" | "MRK" | "LUK" | "JHN" | "ACT" | "ROM" | "1CO" | "2CO" | "GAL" | "EPH" | "PHP"
        | "COL" | "1TH" | "2TH" | "1TI" | "2TI" | "TIT" | "PHM" | "HEB" | "JAS" | "1PE" | "2PE"
        | "1JN" | "2JN" | "3JN" | "JUD" | "REV" => CanonicalSection::NewTestament,
        _ => CanonicalSection::Unknown,
    }
}

fn append_piece(text: &mut String, piece: &str) {
    if piece.is_empty() {
        return;
    }
    if !text.is_empty() {
        let need_space: bool = !text.ends_with([' ', '\n', '\t', '\u{00A0}'])
            && !piece.starts_with([' ', '\n', '\t', '\u{00A0}']);
        if need_space {
            text.push(' ');
        }
    }
    text.push_str(piece);
}

fn pieces_to_spans_and_text(
    pieces: &[RawVersePiece],
) -> (Vec<GenericTextSpan>, String, bool, bool, Vec<String>) {
    let mut spans: Vec<GenericTextSpan> = Vec::new();
    let mut merged_text: String = String::new();
    let mut is_poem: bool = false;
    let mut is_words_of_jesus: bool = false;
    let mut footnote_ids: Vec<String> = Vec::new();
    let mut pos: i32 = 0;

    for piece in pieces {
        let piece_raw_json: Option<String> = serialize_opt_json(Some(piece));
        match piece {
            RawVersePiece::Text(t) => {
                if !t.is_empty() {
                    append_piece(&mut merged_text, t);
                    spans.push(GenericTextSpan {
                        position: pos,
                        kind: SpanKind::Text,
                        text: t.clone(),
                        formatting: SpanFormatting::default(),
                        lexical: None,
                        footnote_ids: Vec::new(),
                        cross_ref_ids: Vec::new(),
                        language_code: None,
                        raw_json: piece_raw_json,
                    });
                    pos += 1;
                }
            }
            RawVersePiece::Formatted(f) => {
                is_poem |= f.poem;
                is_words_of_jesus |= f.words_of_jesus;
                if !f.text.is_empty() {
                    append_piece(&mut merged_text, &f.text);
                    let lexical: Option<LexicalInfo> =
                        f.strong.as_ref().map(|s: &String| LexicalInfo {
                            strongs: Some(s.clone()),
                            ..LexicalInfo::default()
                        });
                    let kind: SpanKind = if lexical.is_some() {
                        SpanKind::Word
                    } else if f.words_of_jesus {
                        SpanKind::Quote
                    } else {
                        SpanKind::Text
                    };
                    spans.push(GenericTextSpan {
                        position: pos,
                        kind,
                        text: f.text.clone(),
                        formatting: SpanFormatting {
                            is_words_of_jesus: f.words_of_jesus,
                            is_poetry: f.poem,
                            ..SpanFormatting::default()
                        },
                        lexical,
                        footnote_ids: Vec::new(),
                        cross_ref_ids: Vec::new(),
                        language_code: None,
                        raw_json: piece_raw_json,
                    });
                    pos += 1;
                }
            }
            RawVersePiece::Footnote(note) => {
                if let Some(id) = &note.note_id {
                    footnote_ids.push(id.clone());
                    spans.push(GenericTextSpan {
                        position: pos,
                        kind: SpanKind::FootnoteRef,
                        text: note.caller.clone().unwrap_or_default(),
                        formatting: SpanFormatting::default(),
                        lexical: None,
                        footnote_ids: vec![id.clone()],
                        cross_ref_ids: Vec::new(),
                        language_code: None,
                        raw_json: piece_raw_json,
                    });
                    pos += 1;
                }
            }
            RawVersePiece::Other(_) => {}
        }
    }

    (spans, merged_text, is_poem, is_words_of_jesus, footnote_ids)
}

fn build_chapter_content(
    book_id: &str,
    chapter_number: i32,
    raw_items: Vec<RawChapterItem>,
    footnotes: Vec<GenericFootnote>,
) -> (
    Vec<GenericChapterItem>,
    Vec<GenericVerse>,
    Vec<GenericFootnote>,
) {
    let mut items: Vec<GenericChapterItem> = Vec::with_capacity(raw_items.len());
    let mut verses: Vec<GenericVerse> = Vec::new();

    for (idx, raw) in raw_items.into_iter().enumerate() {
        let position: i32 = idx as i32;
        let item_raw_json: Option<String> = serialize_opt_json(Some(&raw));
        match raw {
            RawChapterItem::Verse { number, content } => {
                let (spans, text, is_poem, is_words_of_jesus, footnote_ids): (
                    Vec<GenericTextSpan>,
                    String,
                    bool,
                    bool,
                    Vec<String>,
                ) = pieces_to_spans_and_text(&content);
                verses.push(GenericVerse {
                    book_id: book_id.to_string(),
                    chapter: chapter_number,
                    verse: number,
                    text: text.clone(),
                });
                items.push(GenericChapterItem {
                    position,
                    item_type: "verse".to_string(),
                    verse_number: Some(number),
                    text: Some(text),
                    is_poem,
                    is_words_of_jesus,
                    footnote_ids,
                    spans,
                    raw_json: item_raw_json,
                });
            }
            RawChapterItem::Heading { content } => {
                let (spans, text, is_poem, is_words_of_jesus, footnote_ids): (
                    Vec<GenericTextSpan>,
                    String,
                    bool,
                    bool,
                    Vec<String>,
                ) = pieces_to_spans_and_text(&content);
                items.push(GenericChapterItem {
                    position,
                    item_type: "heading".to_string(),
                    verse_number: None,
                    text: Some(text),
                    is_poem,
                    is_words_of_jesus,
                    footnote_ids,
                    spans,
                    raw_json: item_raw_json,
                });
            }
            RawChapterItem::HebrewSubtitle { content } => {
                let (spans, text, is_poem, is_words_of_jesus, footnote_ids): (
                    Vec<GenericTextSpan>,
                    String,
                    bool,
                    bool,
                    Vec<String>,
                ) = pieces_to_spans_and_text(&content);
                items.push(GenericChapterItem {
                    position,
                    item_type: "hebrew_subtitle".to_string(),
                    verse_number: None,
                    text: Some(text),
                    is_poem,
                    is_words_of_jesus,
                    footnote_ids,
                    spans,
                    raw_json: item_raw_json,
                });
            }
            RawChapterItem::LineBreak => {
                items.push(GenericChapterItem {
                    position,
                    item_type: "line_break".to_string(),
                    verse_number: None,
                    text: None,
                    is_poem: false,
                    is_words_of_jesus: false,
                    footnote_ids: Vec::new(),
                    spans: Vec::new(),
                    raw_json: item_raw_json,
                });
            }
            RawChapterItem::Unknown => {
                items.push(GenericChapterItem {
                    position,
                    item_type: "unknown".to_string(),
                    verse_number: None,
                    text: None,
                    is_poem: false,
                    is_words_of_jesus: false,
                    footnote_ids: Vec::new(),
                    spans: Vec::new(),
                    raw_json: item_raw_json,
                });
            }
        }
    }

    (items, verses, footnotes)
}

// ═════════════════════════════════════════════════════════════════════════════
// Flexible Deserializers & Raw HelloAO Wire Structs (Private to Source)
// ═════════════════════════════════════════════════════════════════════════════

fn de_string_from_any<'de, D>(d: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let v: serde_json::Value = Deserialize::deserialize(d)?;
    Ok(match v {
        serde_json::Value::String(s) => s,
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    })
}

fn de_opt_string<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let v: serde_json::Value = Deserialize::deserialize(d)?;
    Ok(match v {
        serde_json::Value::String(s) => Some(s),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Null => None,
        other => Some(other.to_string()),
    })
}

fn de_opt_i32<'de, D>(d: D) -> Result<Option<i32>, D::Error>
where
    D: Deserializer<'de>,
{
    let v: serde_json::Value = Deserialize::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Number(n) => n.as_i64().map(|x: i64| x as i32),
        serde_json::Value::String(s) => s.parse::<i32>().ok(),
        _ => None,
    })
}

#[derive(Deserialize)]
struct TranslationsResponse {
    translations: Vec<RawTranslation>,
}

#[derive(Deserialize)]
struct RawTranslation {
    id: String,
    name: String,
    #[serde(rename = "englishName")]
    english_name: String,
    #[serde(rename = "shortName")]
    short_name: String,
    language: String,
    #[serde(rename = "languageName")]
    language_name: Option<String>,
    #[serde(rename = "languageEnglishName")]
    language_english_name: Option<String>,
    #[serde(rename = "textDirection")]
    text_direction: Option<String>,
    #[serde(rename = "numberOfBooks", default)]
    number_of_books: i32,
    #[serde(rename = "totalNumberOfChapters", default)]
    total_number_of_chapters: i32,
    #[serde(rename = "totalNumberOfVerses", default)]
    total_number_of_verses: i32,
    #[serde(rename = "licenseUrl")]
    license_url: Option<String>,
    #[serde(rename = "licenseNotes")]
    license_notes: Option<String>,
    #[serde(flatten)]
    raw: serde_json::Value,
}

#[derive(Deserialize)]
struct BooksResponse {
    books: Vec<RawBook>,
}

#[derive(Deserialize, Serialize)]
struct RawBook {
    id: String,
    name: String,
    #[serde(rename = "numberOfChapters", default)]
    number_of_chapters: i32,
    #[serde(rename = "commonName")]
    common_name: Option<String>,
    title: Option<String>,
    #[serde(default, deserialize_with = "de_opt_i32")]
    order: Option<i32>,
    #[serde(rename = "audioLinks")]
    audio_links: Option<serde_json::Value>,
    #[serde(rename = "isApocryphal", default)]
    is_apocryphal: Option<bool>,
}

#[derive(Deserialize)]
struct ChapterResponse {
    chapter: RawChapterPayload,
}

#[derive(Deserialize)]
struct SimpleChapterResponse {
    chapter: RawSimpleChapterPayload,
}

#[derive(Deserialize, Serialize)]
struct RawSimpleChapterPayload {
    content: Vec<FlexibleSimpleItem>,
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum FlexibleSimpleItem {
    Verse {
        #[serde(default, deserialize_with = "de_opt_i32")]
        number: Option<i32>,
        #[serde(default, deserialize_with = "de_string_from_any")]
        text: String,
    },
    Other(#[serde(skip_serializing)] IgnoredAny),
}

#[derive(Deserialize)]
struct CompleteResponse {
    books: Vec<RawCompleteBook>,
}

#[derive(Deserialize, Serialize)]
struct RawCompleteBook {
    id: String,
    name: String,
    #[serde(rename = "numberOfChapters", default)]
    number_of_chapters: i32,
    #[serde(rename = "commonName")]
    common_name: Option<String>,
    title: Option<String>,
    #[serde(default, deserialize_with = "de_opt_i32")]
    order: Option<i32>,
    #[serde(rename = "audioLinks")]
    audio_links: Option<serde_json::Value>,
    #[serde(rename = "isApocryphal", default)]
    is_apocryphal: Option<bool>,
    #[serde(default)]
    chapters: Vec<RawCompleteChapterWrapper>,
}

#[derive(Deserialize, Serialize)]
struct RawCompleteChapterWrapper {
    chapter: RawChapterPayload,
}

#[derive(Deserialize, Serialize)]
struct RawChapterPayload {
    #[serde(default)]
    number: i32,
    #[serde(default)]
    content: Vec<RawChapterItem>,
    #[serde(default)]
    footnotes: Vec<RawFootnote>,
    #[serde(rename = "audioTimings", default)]
    audio_timings: Vec<RawAudioTiming>,
}

#[derive(Deserialize, Serialize)]
struct RawFootnote {
    #[serde(rename = "noteId", default, deserialize_with = "de_string_from_any")]
    note_id: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    caller: Option<String>,
    #[serde(default, deserialize_with = "de_string_from_any")]
    text: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    reference: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct RawAudioTiming {
    #[serde(default, deserialize_with = "de_opt_i32")]
    verse: Option<i32>,
    #[serde(rename = "startMs", default)]
    start_ms: Option<i64>,
    #[serde(rename = "endMs", default)]
    end_ms: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type")]
enum RawChapterItem {
    #[serde(rename = "verse")]
    Verse {
        #[serde(default)]
        number: i32,
        #[serde(default)]
        content: Vec<RawVersePiece>,
    },
    #[serde(rename = "heading")]
    Heading {
        #[serde(default)]
        content: Vec<RawVersePiece>,
    },
    #[serde(rename = "hebrew_subtitle")]
    HebrewSubtitle {
        #[serde(default)]
        content: Vec<RawVersePiece>,
    },
    #[serde(rename = "line_break")]
    LineBreak,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
enum RawVersePiece {
    Text(String),
    Formatted(RawFormattedText),
    Footnote(RawInlineFootnote),
    Other(serde_json::Value),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RawFormattedText {
    #[serde(default, deserialize_with = "de_string_from_any")]
    text: String,
    #[serde(default)]
    poem: bool,
    #[serde(rename = "wordsOfJesus", default)]
    words_of_jesus: bool,
    #[serde(default, deserialize_with = "de_opt_string")]
    strong: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RawInlineFootnote {
    #[serde(rename = "noteId", default, deserialize_with = "de_opt_string")]
    note_id: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    caller: Option<String>,
}

#[derive(Deserialize)]
struct CommentariesResponse {
    commentaries: Vec<RawCommentary>,
}

#[derive(Deserialize)]
struct RawCommentary {
    id: String,
    name: String,
    #[serde(rename = "englishName", default)]
    english_name: String,
    #[serde(default)]
    language: String,
    #[serde(rename = "licenseUrl")]
    license_url: Option<String>,
    #[serde(rename = "licenseNotes")]
    license_notes: Option<String>,
    #[serde(rename = "numberOfBooks", default)]
    number_of_books: i32,
    #[serde(rename = "totalNumberOfChapters", default)]
    total_number_of_chapters: i32,
    #[serde(rename = "totalNumberOfVerses", default)]
    total_number_of_verses: i32,
}

#[derive(Deserialize)]
struct ProfilesResponse {
    profiles: Vec<HelloCommentaryProfile>,
}

#[derive(Deserialize)]
struct DatasetsResponse {
    datasets: Vec<RawDataset>,
}

#[derive(Deserialize)]
struct RawDataset {
    id: String,
    name: String,
    #[serde(rename = "englishName")]
    english_name: Option<String>,
    language: Option<String>,
    #[serde(rename = "licenseUrl")]
    license_url: Option<String>,
    #[serde(rename = "licenseNotes")]
    license_notes: Option<String>,
    #[serde(rename = "totalNumberOfBooks")]
    total_number_of_books: Option<i32>,
    #[serde(rename = "totalNumberOfWords")]
    total_number_of_words: Option<i32>,
    #[serde(rename = "totalNumberOfLexicon")]
    total_number_of_lexicon: Option<i32>,
}

#[derive(Deserialize)]
struct RawEntityItem {
    id: String,
    name: String,
    #[serde(rename = "isProperName")]
    is_proper_name: Option<bool>,
    gender: Option<String>,
    #[serde(rename = "startDate", default, deserialize_with = "de_opt_string")]
    start_date: Option<String>,
    #[serde(rename = "numberOfReferences", default)]
    number_of_references: i32,
}

impl From<RawEntityItem> for HelloEntitySummary {
    fn from(r: RawEntityItem) -> Self {
        Self {
            id: r.id,
            name: r.name,
            is_proper_name: r.is_proper_name,
            gender: r.gender,
            start_date: r.start_date,
            number_of_references: r.number_of_references,
        }
    }
}

#[derive(Deserialize)]
struct PeopleListResponse {
    people: Vec<RawEntityItem>,
}

#[derive(Deserialize)]
struct EventsListResponse {
    events: Vec<RawEntityItem>,
}

#[derive(Deserialize)]
struct PlacesListResponse {
    places: Vec<RawEntityItem>,
}

#[derive(Deserialize)]
struct GroupsListResponse {
    groups: Vec<RawEntityItem>,
}

#[derive(Deserialize)]
struct LexiconEntryResponse {
    entry: RawLexiconEntry,
}

#[derive(Deserialize)]
struct RawLexiconEntry {
    id: String,
    #[serde(default)]
    senses: Vec<RawLexiconSense>,
}

#[derive(Deserialize)]
struct RawLexiconSense {
    lemma: Option<String>,
    transliteration: Option<String>,
    #[serde(rename = "partOfSpeech")]
    part_of_speech: Option<String>,
    gloss: Option<String>,
    definition: Option<String>,
}
