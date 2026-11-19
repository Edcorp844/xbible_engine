//! HelloAO HTTP client and raw-JSON-to-generic normalizer (`sources/helloao/client.rs`).
//!
//! All HelloAO-specific wire types, endpoint paths, and JSON parsing quirks
//! (`books.json` `isApocryphal` fallback, `complete.json` streaming download,
//! bounded concurrent chapter fetching over the shared Tokio runtime, dataset
//! count fallbacks, and flexible deserializers) live here and map into the
//! source-agnostic `Generic*` structs consumed by the Store and DB.
//!
//! Wire types are aligned with the live Free Use Bible API:
//!   https://bible.helloao.org/api/...
//!   https://bible.helloao.org/docs/reference/translations/
//!
//! Normalization contract (v11+):
//! - Every chapter item's `content` (string, array-of-strings, array-of-objects,
//!   nested mixes) is flattened via [`flatten_content`] so `text` AND `spans`
//!   are populated for poetry, wisdom, prophecy, genealogies, and prose alike.
//! - Inline markers (`noteId`, `lineBreak`) are skipped and never leak into verse text.
//! - A shape-mismatched `complete.json` errors out of `normalize_complete_books`
//!   and falls back to the concurrent per-chapter fetch instead of persisting
//!   an empty bundle with the `installed = 1` flag set.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use reqwest::blocking::{Client, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::runtime::Runtime;
use tokio::sync::Semaphore;
use tokio::task::{JoinError, JoinSet};

use crate::runtime::get_shared_runtime;
use crate::sources::models::{
    CanonicalSection, GenericAudioSource, GenericAudioTiming, GenericBook, GenericBookChapters,
    GenericCatalogBible, GenericChapter, GenericChapterItem, GenericFootnote,
    GenericInstalledBible, GenericTextSpan, GenericVerse, SpanFormatting, SpanKind, TextDirection,
};
use crate::sources::{InstallPhase, InstallProgressSink};

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

        let RawChapterPayload {
            number,
            content,
            footnotes,
            audio_timings,
        } = data.chapter;

        let effective_chapter: i32 = if number == 0 { chapter_number } else { number };

        let raw_json: String = serialize_json(&RawChapterPayload {
            number: effective_chapter,
            content: content.clone(),
            footnotes: footnotes.clone(),
            audio_timings: audio_timings.clone(),
        });

        let mapped_footnotes: Vec<GenericFootnote> =
            footnotes.into_iter().map(map_footnote).collect();
        let mapped_timings: Vec<GenericAudioTiming> =
            audio_timings.into_iter().map(map_audio_timing).collect();

        let (items, verses, chapter_footnotes) =
            build_chapter_content(book_id, effective_chapter, content, mapped_footnotes);

        Ok(GenericChapter {
            translation_id: translation_id.to_string(),
            source_id: HELLOAO_SOURCE_ID.to_string(),
            book_id: book_id.to_string(),
            chapter_number: effective_chapter,
            items,
            verses,
            footnotes: chapter_footnotes,
            audio_sources: Vec::new(),
            audio_timings: mapped_timings,
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

    /// Downloads and normalizes a complete HelloAO translation into
    /// `GenericInstalledBible`, falling back to bounded concurrent
    /// per-chapter fetches on the shared Tokio runtime when `complete.json`
    /// is unavailable, truncated, shape-mismatched, or contains no books.
    pub fn fetch_for_install(
        &self,
        translation_id: &str,
        progress: &dyn InstallProgressSink,
    ) -> Result<GenericInstalledBible, String> {
        if progress.is_cancelled() {
            return Err("Installation cancelled before start".to_string());
        }

        progress.on_progress(
            InstallPhase::Catalog,
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

        // `books.json` is the authoritative source for `isApocryphal`.
        let books_list: Vec<GenericBook> = self.list_books(translation_id)?;

        if books_list.is_empty() {
            return Err(format!(
                "helloao {}: books.json returned 0 books",
                translation_id
            ));
        }

        let apocryphal_by_id: HashMap<String, bool> = books_list
            .iter()
            .map(|b: &GenericBook| {
                let from_raw: Option<bool> = b
                    .raw_json
                    .as_deref()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                    .and_then(|v| v.get("isApocryphal").and_then(|x| x.as_bool()));
                let flag: bool = from_raw.unwrap_or(matches!(
                    b.section,
                    CanonicalSection::Deuterocanon
                        | CanonicalSection::OrthodoxExtra
                        | CanonicalSection::EthiopianExtra
                ));
                (b.id.clone(), flag)
            })
            .collect();

        progress.on_progress(
            InstallPhase::Download,
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
                    progress.on_progress(InstallPhase::Download, current_u32, total_u32, None);
                }

                if progress.is_cancelled() {
                    return Err("Installation cancelled before normalization".to_string());
                }

                let length_ok: bool = match total {
                    Some(t) if t > 0 => received == t,
                    _ => true,
                };

                if !length_ok {
                    log::warn!(
                        "helloao {}: complete.json truncated ({} / {} bytes); \
                         falling back to concurrent per-chapter fetch",
                        translation_id,
                        received,
                        total.unwrap_or(0)
                    );
                } else {
                    match serde_json::from_slice::<CompleteResponse>(&buf) {
                        Ok(data) => {
                            log::info!(
                                "helloao {}: complete.json parsed — {} books",
                                translation_id,
                                data.books.len()
                            );
                            if data.books.is_empty() {
                                log::warn!(
                                    "helloao {}: complete.json contained 0 books; \
                                     falling back to concurrent per-chapter fetch",
                                    translation_id
                                );
                            } else {
                                progress.on_progress(
                                    InstallPhase::Normalize,
                                    2,
                                    3,
                                    Some("Structuring books, chapters, and verses".to_string()),
                                );

                                match normalize_complete_books(
                                    translation_id,
                                    catalog_entry.clone(),
                                    data.books,
                                    &apocryphal_by_id,
                                ) {
                                    Ok(bundle) => {
                                        progress.on_progress(InstallPhase::Done, 3, 3, None);
                                        return Ok(bundle);
                                    }
                                    Err(e) => {
                                        log::warn!(
                                            "helloao {}: normalize_complete_books failed ({}); \
                                             falling back to concurrent per-chapter fetch",
                                            translation_id,
                                            e
                                        );
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!(
                                "helloao {}: complete.json parse failed ({}); \
                                 falling back to concurrent per-chapter fetch",
                                translation_id,
                                e
                            );
                        }
                    }
                }
            }
        }

        self.fetch_for_install_concurrent(translation_id, catalog_entry, books_list, progress)
    }

    /// Fetches all chapters across `books` concurrently with bounded
    /// parallelism. Failed chapter fetches are collected and returned as an
    /// error — silent skipping is how "missing verses" appeared between
    /// retries in earlier versions.
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

        if total_chapters == 0 {
            return Err(format!(
                "helloao {}: no chapters to fetch (books.json may be empty)",
                translation_id
            ));
        }

        progress.on_progress(
            InstallPhase::Download,
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

        let (fetched_pairs, failures): (Vec<FetchedChapterPair>, Vec<(String, i32, String)>) =
            runtime.block_on(async move {
                let mut join_set: JoinSet<(
                    String,
                    i32,
                    Option<FetchedChapterPair>,
                    Option<String>,
                )> = JoinSet::new();

                for (book, ch_num) in tasks {
                    let permit: tokio::sync::OwnedSemaphorePermit =
                        match Arc::clone(&semaphore).acquire_owned().await {
                            Ok(p) => p,
                            Err(_) => break,
                        };
                    let client_ref: Arc<HelloAOClient> = Arc::clone(&client_arc);
                    let tid_ref: Arc<String> = Arc::clone(&tid_arc);
                    let book_id: String = book.id.clone();

                    join_set.spawn(async move {
                        let _permit: tokio::sync::OwnedSemaphorePermit = permit;
                        let tid_for_fetch: String = (*tid_ref).clone();
                        let book_id_for_fetch: String = book_id.clone();

                        let res: Result<GenericChapter, String> = tokio::task::spawn_blocking(
                            move || -> Result<GenericChapter, String> {
                                client_ref.fetch_chapter(&tid_for_fetch, &book_id_for_fetch, ch_num)
                            },
                        )
                        .await
                        .unwrap_or_else(|e: JoinError| Err(e.to_string()));

                        match res {
                            Ok(chapter) => {
                                let pair: FetchedChapterPair = FetchedChapterPair {
                                    book_id: book_id.clone(),
                                    chapter,
                                };
                                (book_id, ch_num, Some(pair), None)
                            }
                            Err(err) => (book_id, ch_num, None, Some(err)),
                        }
                    });
                }

                let mut results: Vec<FetchedChapterPair> = Vec::new();
                let mut failures: Vec<(String, i32, String)> = Vec::new();
                let mut done: u32 = 0;

                while let Some(joined) = join_set.join_next().await {
                    match joined {
                        Ok((_book_id, _ch_num, Some(pair), _)) => {
                            results.push(pair);
                            done = done.saturating_add(1);
                            progress.on_progress(
                                InstallPhase::Download,
                                done,
                                total_chapters,
                                None,
                            );
                        }
                        Ok((book_id, ch_num, None, Some(err))) => {
                            let message: String = format!("chapter {} {} failed", book_id, ch_num);
                            failures.push((book_id, ch_num, err));
                            done = done.saturating_add(1);
                            progress.on_progress(
                                InstallPhase::Download,
                                done,
                                total_chapters,
                                Some(message),
                            );
                        }
                        Ok(_) => {}
                        Err(join_err) => {
                            failures.push((
                                "?".to_string(),
                                0,
                                format!("task join error: {}", join_err),
                            ));
                        }
                    }
                    if progress.is_cancelled() {
                        join_set.abort_all();
                        break;
                    }
                }

                (results, failures)
            });

        if progress.is_cancelled() {
            return Err("Installation cancelled".to_string());
        }

        if !failures.is_empty() {
            let sample: String = failures
                .iter()
                .take(10)
                .map(|(b, c, e)| format!("{} {}: {}", b, c, e))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(format!(
                "{} of {} chapters failed to download ({}). \
                 Aborting install so it can be retried. First failures: {}",
                failures.len(),
                total_chapters,
                if failures.len() > 10 {
                    "truncated"
                } else {
                    "all shown"
                },
                sample
            ));
        }

        if fetched_pairs.is_empty() {
            return Err(format!(
                "helloao {}: concurrent fetch returned 0 chapters",
                translation_id
            ));
        }

        progress.on_progress(
            InstallPhase::Normalize,
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

        let total_verses: usize = chapters_by_book
            .iter()
            .flat_map(|g| g.chapters.iter())
            .map(|c| c.verses.len())
            .sum();
        log::info!(
            "helloao {}: concurrent fetch normalized {} chapters, {} verses",
            translation_id,
            chapters_by_book
                .iter()
                .map(|g| g.chapters.len())
                .sum::<usize>(),
            total_verses
        );

        progress.on_progress(InstallPhase::Done, 3, 3, None);

        Ok(GenericInstalledBible {
            catalog: catalog_entry,
            books,
            chapters_by_book,
        })
    }

    // ── Commentaries ──────────────────────────────────────────────────────────

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

        let RawChapterPayload {
            number,
            content,
            footnotes,
            audio_timings,
        } = data.chapter;

        let ch_num: i32 = if number == 0 {
            chapter_number as i32
        } else {
            number
        };

        let raw_json: String = serialize_json(&RawChapterPayload {
            number: ch_num,
            content: content.clone(),
            footnotes: footnotes.clone(),
            audio_timings: audio_timings.clone(),
        });

        let mapped_footnotes: Vec<GenericFootnote> =
            footnotes.into_iter().map(map_footnote).collect();
        let mapped_timings: Vec<GenericAudioTiming> =
            audio_timings.into_iter().map(map_audio_timing).collect();

        let (items, verses, chapter_footnotes) =
            build_chapter_content(book_id, ch_num, content, mapped_footnotes);

        Ok(GenericChapter {
            translation_id: commentary_id.to_string(),
            source_id: HELLOAO_SOURCE_ID.to_string(),
            book_id: book_id.to_string(),
            chapter_number: ch_num,
            items,
            verses,
            footnotes: chapter_footnotes,
            audio_sources: Vec::new(),
            audio_timings: mapped_timings,
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

    // ── Datasets ──────────────────────────────────────────────────────────────

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
            lemma: sense.as_ref().and_then(|s| s.lemma.clone()),
            transliteration: sense.as_ref().and_then(|s| s.transliteration.clone()),
            part_of_speech: sense.as_ref().and_then(|s| s.part_of_speech.clone()),
            gloss: sense.as_ref().and_then(|s| s.gloss.clone()),
            definition: sense.as_ref().and_then(|s| s.definition.clone()),
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
// HelloEntitySummary conversions
// ═════════════════════════════════════════════════════════════════════════════

impl From<RawPersonSummary> for HelloEntitySummary {
    fn from(p: RawPersonSummary) -> Self {
        Self {
            id: p.id,
            name: p.name,
            is_proper_name: p.is_proper_name,
            gender: p.gender,
            start_date: None,
            number_of_references: p.number_of_references.unwrap_or(0),
        }
    }
}

impl From<RawEventSummary> for HelloEntitySummary {
    fn from(e: RawEventSummary) -> Self {
        Self {
            id: e.id,
            name: e.name,
            is_proper_name: None,
            gender: None,
            start_date: e.start_date,
            number_of_references: e.number_of_references.unwrap_or(0),
        }
    }
}

impl From<RawPlaceSummary> for HelloEntitySummary {
    fn from(p: RawPlaceSummary) -> Self {
        Self {
            id: p.id,
            name: p.name,
            is_proper_name: None,
            gender: None,
            start_date: None,
            number_of_references: p.number_of_references.unwrap_or(0),
        }
    }
}

impl From<RawGroupSummary> for HelloEntitySummary {
    fn from(g: RawGroupSummary) -> Self {
        Self {
            id: g.id,
            name: g.name,
            is_proper_name: None,
            gender: None,
            start_date: None,
            number_of_references: g.number_of_references.unwrap_or(0),
        }
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

fn json_value_to_string(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        other => Some(other.to_string()),
    }
}

fn map_footnote(f: RawFootnote) -> GenericFootnote {
    let note_id: String = json_value_to_string(&f.note_id).unwrap_or_default();
    let caller: Option<String> = json_value_to_string(&f.caller);
    let text: Option<String> = json_value_to_string(&f.text);
    let reference: Option<String> = json_value_to_string(&f.reference);
    let note_raw_json: Option<String> = serialize_opt_json(Some(&f));

    GenericFootnote {
        note_id,
        caller,
        text,
        reference,
        raw_json: note_raw_json,
    }
}

fn map_audio_timing(t: RawAudioTiming) -> GenericAudioTiming {
    GenericAudioTiming {
        verse: t.verse.unwrap_or(0),
        start_ms: t.start_ms,
        end_ms: t.end_ms,
    }
}

fn normalize_complete_books(
    translation_id: &str,
    catalog: GenericCatalogBible,
    raw_books: Vec<RawCompleteBook>,
    apocryphal_by_id: &HashMap<String, bool>,
) -> Result<GenericInstalledBible, String> {
    let mut books: Vec<GenericBook> = Vec::with_capacity(raw_books.len());
    let mut chapters_by_book: Vec<GenericBookChapters> = Vec::with_capacity(raw_books.len());
    let mut total_chapters: usize = 0;
    let mut total_verses: usize = 0;

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
            audio_sources: book_audio,
            audio_links_json,
            raw_json: book_raw_json,
        });

        let mut book_chapters: Vec<GenericChapter> = Vec::with_capacity(b.chapters.len());
        for wrap in b.chapters {
            let RawChapterPayload {
                number,
                content,
                footnotes,
                audio_timings,
            } = wrap.chapter;

            let chapter_number: i32 = if number == 0 { 0 } else { number };
            let chapter_raw_json: String = serialize_json(&RawChapterPayload {
                number: chapter_number,
                content: content.clone(),
                footnotes: footnotes.clone(),
                audio_timings: audio_timings.clone(),
            });

            let mapped_footnotes: Vec<GenericFootnote> =
                footnotes.into_iter().map(map_footnote).collect();
            let mapped_timings: Vec<GenericAudioTiming> =
                audio_timings.into_iter().map(map_audio_timing).collect();

            let (items, verses, chapter_footnotes) =
                build_chapter_content(&b.id, chapter_number, content, mapped_footnotes);

            total_chapters += 1;
            total_verses += verses.len();

            book_chapters.push(GenericChapter {
                translation_id: translation_id.to_string(),
                source_id: HELLOAO_SOURCE_ID.to_string(),
                book_id: b.id.clone(),
                chapter_number,
                items,
                verses,
                footnotes: chapter_footnotes,
                audio_sources: Vec::new(),
                audio_timings: mapped_timings,
                raw_json: chapter_raw_json,
            });
        }

        book_chapters.sort_by_key(|c: &GenericChapter| c.chapter_number);
        chapters_by_book.push(GenericBookChapters {
            book_id: b.id,
            chapters: book_chapters,
        });
    }

    if books.is_empty() || total_chapters == 0 {
        return Err(format!(
            "normalize_complete_books produced {} books / {} chapters for {} — \
             complete.json was likely malformed",
            books.len(),
            total_chapters,
            translation_id
        ));
    }

    log::info!(
        "helloao {}: complete.json normalized {} books, {} chapters, {} verses",
        translation_id,
        books.len(),
        total_chapters,
        total_verses
    );

    Ok(GenericInstalledBible {
        catalog,
        books,
        chapters_by_book,
    })
}

fn parse_text_direction(raw: Option<&str>) -> TextDirection {
    match raw {
        Some("rtl") | Some("RTL") | Some("right-to-left") | Some("right_to_left") => {
            TextDirection::RightToLeft
        }
        _ => TextDirection::LeftToRight,
    }
}

fn parse_translation_audio_sources(raw: &serde_json::Value) -> Vec<GenericAudioSource> {
    let mut out: Vec<GenericAudioSource> = Vec::new();
    let audio_val: Option<&serde_json::Value> = raw.get("audio");

    let push = |obj: &serde_json::Value, out: &mut Vec<GenericAudioSource>| {
        let id: String = obj
            .get("id")
            .or_else(|| obj.get("filesetId"))
            .or_else(|| obj.get("fileset_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            return;
        }
        out.push(GenericAudioSource {
            source_ref_id: id,
            url: obj.get("url").and_then(|v| v.as_str()).map(String::from),
            format: obj.get("format").and_then(|v| v.as_str()).map(String::from),
            is_dramatized: obj.get("isDramatized").and_then(|v| v.as_bool()),
        });
    };

    if let Some(v) = audio_val {
        match v {
            serde_json::Value::Array(arr) => {
                for item in arr {
                    push(item, &mut out);
                }
            }
            serde_json::Value::Object(_) => push(v, &mut out),
            _ => {}
        }
    }

    out
}

fn parse_audio_sources(audio_links: Option<&serde_json::Value>) -> Vec<GenericAudioSource> {
    let Some(v) = audio_links else {
        return Vec::new();
    };
    let mut out: Vec<GenericAudioSource> = Vec::new();

    let push = |obj: &serde_json::Value, out: &mut Vec<GenericAudioSource>| {
        let id: String = obj
            .get("id")
            .or_else(|| obj.get("filesetId"))
            .or_else(|| obj.get("fileset_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            return;
        }
        out.push(GenericAudioSource {
            source_ref_id: id,
            url: obj.get("url").and_then(|v| v.as_str()).map(String::from),
            format: obj.get("format").and_then(|v| v.as_str()).map(String::from),
            is_dramatized: obj.get("isDramatized").and_then(|v| v.as_bool()),
        });
    };

    match v {
        serde_json::Value::Array(arr) => {
            for item in arr {
                push(item, &mut out);
            }
        }
        serde_json::Value::Object(_) => push(v, &mut out),
        _ => {}
    }

    out
}

fn infer_section(book_id: &str, is_apocryphal: bool) -> CanonicalSection {
    let id: String = book_id.to_uppercase();

    const ETHIOPIAN_EXTRA: &[&str] = &["ENO", "JUB", "1MQ", "2MQ", "3MQ", "4BA"];
    const ORTHODOX_EXTRA: &[&str] = &["3MA", "4MA", "DAG", "ADE"];

    if ETHIOPIAN_EXTRA.contains(&id.as_str()) {
        return CanonicalSection::EthiopianExtra;
    }
    if ORTHODOX_EXTRA.contains(&id.as_str()) {
        return CanonicalSection::OrthodoxExtra;
    }

    if !is_apocryphal {
        const NT: &[&str] = &[
            "MAT", "MRK", "LUK", "JHN", "ACT", "ROM", "1CO", "2CO", "GAL", "EPH", "PHP", "COL",
            "1TH", "2TH", "1TI", "2TI", "TIT", "PHM", "HEB", "JAS", "1PE", "2PE", "1JN", "2JN",
            "3JN", "JUD", "REV",
        ];
        if NT.contains(&id.as_str()) {
            return CanonicalSection::NewTestament;
        }
        return CanonicalSection::OldTestament;
    }

    CanonicalSection::Deuterocanon
}

// ═════════════════════════════════════════════════════════════════════════════
// Chapter content normalization
// ═════════════════════════════════════════════════════════════════════════════
//
// HelloAO encodes chapter `content` items in several shapes:
//
//   1. Plain string:           "content": "In the beginning..."
//   2. Array of strings:       "content": ["line one", "line two"]
//   3. Array of line objects:  "content": [{"text": "...", "poem": 1}, ...]
//   4. Nested mixes + markers: {"noteId": 0}, {"lineBreak": true, "paragraph": "p"}
//
// Inline markers must be skipped so they never leak digits / noise into verse text.

struct ExtractedPiece {
    text: String,
    is_poetry: bool,
    is_words_of_jesus: bool,
}

/// Recursively walk any `serde_json::Value` and collect visible text pieces.
fn collect_pieces(value: &serde_json::Value, out: &mut Vec<ExtractedPiece>) {
    match value {
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
        serde_json::Value::String(s) => {
            if !s.is_empty() {
                out.push(ExtractedPiece {
                    text: s.clone(),
                    is_poetry: false,
                    is_words_of_jesus: false,
                });
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                collect_pieces(item, out);
            }
        }
        serde_json::Value::Object(obj) => {
            // Skip pure inline markers — they carry navigation / footnote refs, not prose.
            if obj.contains_key("noteId")
                || obj.contains_key("note_id")
                || obj.contains_key("lineBreak")
                || obj.contains_key("line_break")
            {
                return;
            }

            let is_poetry: bool = obj.get("poem").map(|v| !v.is_null()).unwrap_or(false);
            let is_words_of_jesus: bool = obj
                .get("wordsOfJesus")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                || obj
                    .get("words_of_jesus")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

            if let Some(t) = obj.get("text").and_then(|v| v.as_str()) {
                if !t.is_empty() {
                    out.push(ExtractedPiece {
                        text: t.to_string(),
                        is_poetry,
                        is_words_of_jesus,
                    });
                }
            }

            if let Some(nested) = obj.get("content") {
                collect_pieces(nested, out);
            }
            if let Some(spans) = obj.get("spans") {
                collect_pieces(spans, out);
            }
        }
    }
}

/// Flatten a raw chapter item's `content` into `(text, spans, is_poem, is_words_of_jesus)`.
fn flatten_content(
    content: &serde_json::Value,
    base_position: i32,
) -> (String, Vec<GenericTextSpan>, bool, bool) {
    let mut pieces: Vec<ExtractedPiece> = Vec::new();
    collect_pieces(content, &mut pieces);

    if pieces.is_empty() {
        return (String::new(), Vec::new(), false, false);
    }

    let mut joined: String = String::new();
    let mut spans: Vec<GenericTextSpan> = Vec::with_capacity(pieces.len());
    let mut any_poetry: bool = false;
    let mut any_words_of_jesus: bool = false;

    for (idx, piece) in pieces.into_iter().enumerate() {
        if !joined.is_empty() {
            joined.push(' ');
        }
        joined.push_str(&piece.text);

        if piece.is_poetry {
            any_poetry = true;
        }
        if piece.is_words_of_jesus {
            any_words_of_jesus = true;
        }

        spans.push(GenericTextSpan {
            position: base_position + idx as i32,
            kind: SpanKind::Text,
            text: piece.text,
            formatting: SpanFormatting {
                is_words_of_jesus: piece.is_words_of_jesus,
                is_poetry: piece.is_poetry,
                ..Default::default()
            },
            lexical: None,
            footnote_ids: Vec::new(),
            cross_ref_ids: Vec::new(),
            language_code: None,
            raw_json: None,
        });
    }

    (joined, spans, any_poetry, any_words_of_jesus)
}

fn raw_verse_number(item: &serde_json::Value) -> Option<i32> {
    item.get("number")
        .and_then(|v| v.as_i64())
        .map(|n| n as i32)
        .or_else(|| item.get("verse").and_then(|v| v.as_i64()).map(|n| n as i32))
}

fn raw_item_type(item: &serde_json::Value, has_number: bool) -> String {
    if let Some(t) = item.get("type").and_then(|v| v.as_str()) {
        return t.to_string();
    }
    if has_number {
        "verse".to_string()
    } else {
        "text".to_string()
    }
}

fn raw_footnote_ids(item: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    if let Some(arr) = item.get("footnotes").and_then(|v| v.as_array()) {
        for f in arr {
            match f {
                serde_json::Value::String(s) => out.push(s.clone()),
                serde_json::Value::Object(o) => {
                    if let Some(id) = o.get("noteId").and_then(|v| v.as_i64()) {
                        out.push(id.to_string());
                    } else if let Some(id) = o.get("noteId").and_then(|v| v.as_str()) {
                        out.push(id.to_string());
                    } else if let Some(id) = o.get("id").and_then(|v| v.as_str()) {
                        out.push(id.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    // Inline `{"noteId": N}` markers inside content also reference footnotes.
    if let Some(arr) = item.get("content").and_then(|v| v.as_array()) {
        for c in arr {
            if let Some(id) = c.get("noteId").and_then(|v| v.as_i64()) {
                out.push(id.to_string());
            } else if let Some(id) = c.get("noteId").and_then(|v| v.as_str()) {
                out.push(id.to_string());
            }
        }
    }

    out
}

/// Build normalized chapter content (items + verses + merged footnotes).
fn build_chapter_content(
    book_id: &str,
    chapter_number: i32,
    raw_content: Vec<serde_json::Value>,
    mut footnotes: Vec<GenericFootnote>,
) -> (
    Vec<GenericChapterItem>,
    Vec<GenericVerse>,
    Vec<GenericFootnote>,
) {
    let mut items: Vec<GenericChapterItem> = Vec::with_capacity(raw_content.len());
    let mut verse_map: BTreeMap<i32, String> = BTreeMap::new();
    let mut item_position: i32 = 0;

    for raw in raw_content {
        let verse_number: Option<i32> = raw_verse_number(&raw);
        let item_type: String = raw_item_type(&raw, verse_number.is_some());

        let content_value: &serde_json::Value =
            raw.get("content").unwrap_or(&serde_json::Value::Null);

        let (joined_text, spans, is_poetry, is_words_of_jesus) = flatten_content(content_value, 0);

        let footnote_ids: Vec<String> = raw_footnote_ids(&raw);

        let text_field: Option<String> = if joined_text.is_empty() {
            None
        } else {
            Some(joined_text.clone())
        };

        let item_raw_json: Option<String> = serialize_opt_json(Some(&raw));

        items.push(GenericChapterItem {
            position: item_position,
            item_type,
            verse_number,
            text: text_field,
            is_poem: is_poetry,
            is_words_of_jesus,
            footnote_ids,
            spans,
            raw_json: item_raw_json,
        });
        item_position += 1;

        if let (Some(vnum), false) = (verse_number, joined_text.is_empty()) {
            verse_map
                .entry(vnum)
                .and_modify(|existing: &mut String| {
                    if !existing.is_empty() {
                        existing.push(' ');
                    }
                    existing.push_str(&joined_text);
                })
                .or_insert(joined_text);
        }
    }

    let verses: Vec<GenericVerse> = verse_map
        .into_iter()
        .map(|(verse, text)| GenericVerse {
            book_id: book_id.to_string(),
            chapter: chapter_number,
            verse,
            text,
        })
        .collect();

    let mut seen: HashSet<String> = HashSet::new();
    footnotes.retain(|f: &GenericFootnote| seen.insert(f.note_id.clone()));

    (items, verses, footnotes)
}

// ═════════════════════════════════════════════════════════════════════════════
// Raw Wire Types (serde) — aligned with bible.helloao.org
// ═════════════════════════════════════════════════════════════════════════════
//
// All camelCase field names from the live API are renamed explicitly.
// Optional extras (sha256, availableFormats, license object, audio links, …)
// are captured via `#[serde(flatten)] raw` where useful.

#[derive(Debug, Deserialize)]
struct TranslationsResponse {
    translations: Vec<RawTranslation>,
}

#[derive(Debug, Deserialize)]
struct RawTranslation {
    id: String,
    name: String,
    #[serde(default, rename = "englishName")]
    english_name: String,
    #[serde(default, rename = "shortName")]
    short_name: String,
    #[serde(default)]
    language: String,
    #[serde(default, rename = "languageName")]
    language_name: Option<String>,
    #[serde(default, rename = "languageEnglishName")]
    language_english_name: Option<String>,
    #[serde(default, rename = "textDirection")]
    text_direction: Option<String>,
    #[serde(default, rename = "numberOfBooks")]
    number_of_books: i32,
    #[serde(default, rename = "totalNumberOfChapters")]
    total_number_of_chapters: i32,
    #[serde(default, rename = "totalNumberOfVerses")]
    total_number_of_verses: i32,
    #[serde(default, rename = "licenseUrl")]
    license_url: Option<String>,
    #[serde(default, rename = "licenseNotes")]
    license_notes: Option<String>,
    /// Catch-all: website, license object, sha256, availableFormats, audio, links, …
    #[serde(flatten)]
    raw: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct BooksResponse {
    #[serde(default)]
    translation: Option<RawTranslation>,
    books: Vec<RawBook>,
}

#[derive(Debug, Deserialize, Serialize)]
struct RawBook {
    id: String,
    #[serde(default, rename = "translationId")]
    translation_id: Option<String>,
    name: String,
    #[serde(default, rename = "commonName")]
    common_name: Option<String>,
    #[serde(default, rename = "shortName")]
    short_name: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    order: Option<i32>,
    #[serde(default, rename = "numberOfChapters")]
    number_of_chapters: i32,
    #[serde(default, rename = "firstChapterNumber")]
    first_chapter_number: Option<i32>,
    #[serde(default, rename = "lastChapterNumber")]
    last_chapter_number: Option<i32>,
    #[serde(default, rename = "totalNumberOfVerses")]
    total_number_of_verses: Option<i32>,
    /// Omitted on pure Protestant canons; present on deuterocanonical books.
    #[serde(default, rename = "isApocryphal")]
    is_apocryphal: Option<bool>,
    #[serde(default, rename = "audioLinks")]
    audio_links: Option<serde_json::Value>,
    #[serde(default)]
    sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChapterResponse {
    chapter: RawChapterPayload,
    #[serde(default, rename = "thisChapterAudioLinks")]
    this_chapter_audio_links: Option<serde_json::Value>,
    #[serde(default, rename = "thisChapterAudioTimings")]
    this_chapter_audio_timings: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RawChapterPayload {
    #[serde(default)]
    number: i32,
    /// Heterogeneous: heading | line_break | verse | hebrew_subtitle + nested text.
    content: Vec<serde_json::Value>,
    #[serde(default)]
    footnotes: Vec<RawFootnote>,
    #[serde(default, rename = "audioTimings")]
    audio_timings: Vec<RawAudioTiming>,
}

/// Footnote records use integer `noteId`; accept int or string via Value.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct RawFootnote {
    #[serde(default, rename = "noteId", alias = "id")]
    note_id: serde_json::Value,
    #[serde(default)]
    caller: serde_json::Value,
    #[serde(default)]
    text: serde_json::Value,
    #[serde(default)]
    reference: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RawAudioTiming {
    #[serde(default)]
    verse: Option<i32>,
    #[serde(default, rename = "startMs")]
    start_ms: Option<i64>,
    #[serde(default, rename = "endMs")]
    end_ms: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct SimpleChapterResponse {
    chapter: RawSimpleChapter,
}

#[derive(Debug, Deserialize, Serialize)]
struct RawSimpleChapter {
    #[serde(default)]
    content: Vec<FlexibleSimpleItem>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum FlexibleSimpleItem {
    Verse {
        #[serde(default)]
        number: Option<i32>,
        #[serde(default)]
        text: String,
    },
    Other(serde_json::Value),
}

/// `/api/{translation}/complete.json`
#[derive(Debug, Deserialize)]
struct CompleteResponse {
    #[serde(default)]
    translation: Option<RawTranslation>,
    books: Vec<RawCompleteBook>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct RawCompleteBook {
    id: String,
    name: String,
    #[serde(default, rename = "commonName")]
    common_name: Option<String>,
    #[serde(default, rename = "shortName")]
    short_name: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    order: Option<i32>,
    #[serde(default, rename = "numberOfChapters")]
    number_of_chapters: i32,
    #[serde(default, rename = "totalNumberOfVerses")]
    total_number_of_verses: Option<i32>,
    #[serde(default, rename = "isApocryphal")]
    is_apocryphal: Option<bool>,
    #[serde(default, rename = "audioLinks")]
    audio_links: Option<serde_json::Value>,
    #[serde(default)]
    chapters: Vec<RawCompleteChapterWrap>,
}

/// Each entry in `books[].chapters[]` from complete.json:
/// `{ "numberOfVerses": N, "thisChapterAudioLinks": {...}, "chapter": {...} }`
#[derive(Debug, Clone, Deserialize, Serialize)]
struct RawCompleteChapterWrap {
    #[serde(default, rename = "numberOfVerses")]
    number_of_verses: Option<i32>,
    #[serde(default, rename = "thisChapterAudioLinks")]
    this_chapter_audio_links: Option<serde_json::Value>,
    #[serde(default, rename = "thisChapterAudioTimings")]
    this_chapter_audio_timings: Option<serde_json::Value>,
    chapter: RawChapterPayload,
}

// ── Commentaries ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct CommentariesResponse {
    #[serde(default)]
    commentaries: Vec<RawCommentary>,
}

#[derive(Debug, Deserialize)]
struct RawCommentary {
    id: String,
    name: String,
    #[serde(default, rename = "englishName")]
    english_name: String,
    #[serde(default)]
    language: String,
    #[serde(default, rename = "licenseUrl")]
    license_url: Option<String>,
    #[serde(default, rename = "licenseNotes")]
    license_notes: Option<String>,
    #[serde(default, rename = "numberOfBooks")]
    number_of_books: i32,
    #[serde(default, rename = "totalNumberOfChapters")]
    total_number_of_chapters: i32,
    #[serde(default, rename = "totalNumberOfVerses")]
    total_number_of_verses: i32,
}

#[derive(Debug, Deserialize)]
struct ProfilesResponse {
    #[serde(default)]
    profiles: Vec<HelloCommentaryProfile>,
}

// ── Datasets ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct DatasetsResponse {
    #[serde(default)]
    datasets: Vec<RawDataset>,
}

#[derive(Debug, Deserialize)]
struct RawDataset {
    id: String,
    name: String,
    #[serde(default, rename = "englishName")]
    english_name: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default, rename = "licenseUrl")]
    license_url: Option<String>,
    #[serde(default, rename = "licenseNotes")]
    license_notes: Option<String>,
    #[serde(default, rename = "totalNumberOfBooks")]
    total_number_of_books: Option<i32>,
    #[serde(default, rename = "totalNumberOfWords")]
    total_number_of_words: Option<i32>,
    #[serde(default, rename = "totalNumberOfLexicon")]
    total_number_of_lexicon: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct PeopleListResponse {
    #[serde(default)]
    people: Vec<RawPersonSummary>,
}

#[derive(Debug, Deserialize)]
struct EventsListResponse {
    #[serde(default)]
    events: Vec<RawEventSummary>,
}

#[derive(Debug, Deserialize)]
struct PlacesListResponse {
    #[serde(default)]
    places: Vec<RawPlaceSummary>,
}

#[derive(Debug, Deserialize)]
struct GroupsListResponse {
    #[serde(default)]
    groups: Vec<RawGroupSummary>,
}

#[derive(Debug, Deserialize)]
struct RawPersonSummary {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "isProperName")]
    is_proper_name: Option<bool>,
    #[serde(default)]
    gender: Option<String>,
    #[serde(default, rename = "numberOfReferences")]
    number_of_references: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct RawEventSummary {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "startDate")]
    start_date: Option<String>,
    #[serde(default, rename = "numberOfReferences")]
    number_of_references: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct RawPlaceSummary {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "numberOfReferences")]
    number_of_references: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct RawGroupSummary {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default, rename = "numberOfReferences")]
    number_of_references: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct LexiconEntryResponse {
    entry: RawLexiconEntry,
}

#[derive(Debug, Deserialize)]
struct RawLexiconEntry {
    #[serde(default)]
    id: String,
    #[serde(default)]
    senses: Vec<RawLexiconSense>,
}

#[derive(Debug, Deserialize)]
struct RawLexiconSense {
    #[serde(default)]
    lemma: Option<String>,
    #[serde(default)]
    transliteration: Option<String>,
    #[serde(default, rename = "partOfSpeech")]
    part_of_speech: Option<String>,
    #[serde(default)]
    gloss: Option<String>,
    #[serde(default)]
    definition: Option<String>,
}
