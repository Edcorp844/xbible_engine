//! Store catalog index, shelves, facets, search, and item lookup (`src/features/store/store.rs`).
//!
//! Queries every registered `BibleSource` in `SourceRegistry` for its catalog
//! items normalized as `GenericCatalogBible`, optionally overlays HelloAO-specific
//! commentaries and datasets when `HelloAOSource` is present, and falls back to
//! generic cached catalog rows in `db::catalog` when offline.

use std::collections::HashMap;
use std::sync::Arc;

use log::error;
use rusqlite::Connection;

use crate::db::catalog;
use crate::sources::helloao::HelloAOSource;
use crate::sources::helloao::client::{HelloCommentaryMeta, HelloDatasetMeta};
use crate::sources::{BibleSource, GenericCatalogBible, SourceRegistry};

// ── UniFFI Wire Records ───────────────────────────────────────────────────────

#[derive(Debug, Clone, uniffi::Record)]
pub struct LanguageFacet {
    pub code: String,
    pub name: String,
    pub english_name: String,
    pub translation_count: i32,
    pub commentary_count: i32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct StoreItem {
    pub id: String,
    pub source_id: String,
    pub kind: String,
    pub name: String,
    pub english_name: String,
    pub short_name: String,
    pub language: String,
    pub language_name: Option<String>,
    pub description: String,
    pub badge: Option<String>,
    pub installed: bool,
    pub has_strongs: bool,
    pub has_audio: bool,
    pub book_count: i32,
    pub chapter_count: i32,
    pub verse_count: i32,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct StoreShelf {
    pub title: String,
    pub subtitle: Option<String>,
    pub items: Vec<StoreItem>,
}

// ── Shelf Identifiers ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShelfId {
    Featured,
    BibleReference,
    WordStudy,
    PublicDomain,
    Commentaries,
    StudyTools,
    AllResources,
}

impl ShelfId {
    pub const ALL: &'static [ShelfId] = &[
        Self::Featured,
        Self::BibleReference,
        Self::WordStudy,
        Self::PublicDomain,
        Self::Commentaries,
        Self::StudyTools,
        Self::AllResources,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Featured => "Featured",
            Self::BibleReference => "Bible Reference",
            Self::WordStudy => "Word Study & Interlinear",
            Self::PublicDomain => "Public Domain Classics",
            Self::Commentaries => "Commentaries",
            Self::StudyTools => "Study Datasets",
            Self::AllResources => "All Resources",
        }
    }

    pub fn subtitle(self) -> Option<&'static str> {
        match self {
            Self::Featured => Some("Essential translations, commentaries, and study tools"),
            Self::BibleReference => Some("People, events, places, and groups from Scripture"),
            Self::WordStudy => Some("Translations with Strong's or lexical tagging"),
            Self::PublicDomain => Some("Freely distributable English classics"),
            Self::Commentaries => Some("Verse-by-verse exposition"),
            Self::StudyTools => Some("Lexicons and cross-reference datasets"),
            Self::AllResources => None,
        }
    }

    pub fn from_title(title: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|s: &Self| s.title() == title)
    }
}

// ── Store Index ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Store {
    translations: Vec<GenericCatalogBible>,
    commentaries: Vec<HelloCommentaryMeta>,
    datasets: Vec<HelloDatasetMeta>,
    languages: Vec<LanguageFacet>,
}

impl Store {
    /// Asks each registered source in `registry` for its catalog via `list_catalog()`,
    /// falling back to cached rows in `db::catalog` if no source succeeds.
    pub fn fetch_index(
        registry: &SourceRegistry,
        helloao: Option<&HelloAOSource>,
        conn: Option<&Connection>,
    ) -> Result<Self, String> {
        let sources: Vec<Arc<dyn BibleSource>> = registry.all();
        let mut translations: Vec<GenericCatalogBible> = Vec::new();
        let mut any_source_ok: bool = false;

        for source in sources {
            match source.list_catalog() {
                Ok(mut items) => {
                    any_source_ok = true;
                    translations.append(&mut items);
                }
                Err(err) => {
                    error!(
                        "source {} catalog fetch failed: {}",
                        source.source_id(),
                        err
                    );
                }
            }
        }

        let commentaries: Vec<HelloCommentaryMeta> = helloao
            .and_then(|h: &HelloAOSource| h.commentaries().ok())
            .unwrap_or_default();

        let datasets: Vec<HelloDatasetMeta> = helloao
            .and_then(|h: &HelloAOSource| h.datasets().ok())
            .unwrap_or_default();

        if any_source_ok {
            if let Some(conn) = conn {
                if let Err(err) = catalog::save_translations(conn, &translations) {
                    error!("save_translations failed: {}", err);
                }
            }
            return Ok(Self::from_parts(translations, commentaries, datasets));
        }

        let conn: &Connection = conn.ok_or("network failed and no local database")?;
        let cached_translations: Vec<GenericCatalogBible> =
            catalog::load_translations(conn).map_err(|e| format!("{e:?}"))?;

        if cached_translations.is_empty() {
            return Err("network failed and local database is empty".to_string());
        }

        Ok(Self::from_parts(
            cached_translations,
            commentaries,
            datasets,
        ))
    }

    pub fn from_parts(
        translations: Vec<GenericCatalogBible>,
        commentaries: Vec<HelloCommentaryMeta>,
        datasets: Vec<HelloDatasetMeta>,
    ) -> Self {
        let languages: Vec<LanguageFacet> = Self::build_facets(&translations, &commentaries);
        Self {
            translations,
            commentaries,
            datasets,
            languages,
        }
    }

    // ── Feed & Shelf Paging ───────────────────────────────────────────────────

    pub fn home_feed(&self, language: Option<&str>) -> Vec<StoreShelf> {
        ShelfId::ALL
            .iter()
            .map(|&shelf| self.shelf(shelf, language))
            .filter(|s: &StoreShelf| !s.items.is_empty())
            .collect()
    }

    pub fn shelf(&self, shelf: ShelfId, language: Option<&str>) -> StoreShelf {
        let items: Vec<StoreItem> = self.shelf_items(shelf, language);
        let subtitle: Option<String> = shelf.subtitle().map(str::to_string).or_else(|| {
            if shelf == ShelfId::AllResources {
                Some(format!("{} items", items.len()))
            } else {
                None
            }
        });
        StoreShelf {
            title: shelf.title().to_string(),
            subtitle,
            items,
        }
    }

    pub fn shelf_page(
        &self,
        shelf_id: &str,
        language: Option<&str>,
        offset: u32,
        limit: u32,
    ) -> Vec<StoreItem> {
        let Some(shelf) = ShelfId::from_title(shelf_id) else {
            return Vec::new();
        };
        self.shelf_items(shelf, language)
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect()
    }

    pub fn shelf_total(&self, shelf_id: &str, language: Option<&str>) -> u32 {
        let Some(shelf) = ShelfId::from_title(shelf_id) else {
            return 0;
        };
        self.shelf_items(shelf, language).len() as u32
    }

    fn shelf_items(&self, shelf: ShelfId, language: Option<&str>) -> Vec<StoreItem> {
        match shelf {
            ShelfId::Featured => self.items_featured(language),
            ShelfId::BibleReference => self.items_entities(language),
            ShelfId::WordStudy => self.items_interlinear(language),
            ShelfId::PublicDomain => self.items_public_domain(language),
            ShelfId::Commentaries => self.items_commentaries(language),
            ShelfId::StudyTools => self.items_datasets(language),
            ShelfId::AllResources => self.items_all(language),
        }
    }

    fn items_featured(&self, language: Option<&str>) -> Vec<StoreItem> {
        const FEATURED: &[(&str, &str)] = &[
            ("translation", "BSB"),
            ("translation", "eng_kjv"),
            ("translation", "eng_web"),
            ("commentary", "adam-clarke"),
            ("commentary", "tyndale"),
            ("dataset", "stepbible-lexicon"),
        ];
        FEATURED
            .iter()
            .filter_map(|(kind, id): &(&str, &str)| self.item(id, kind))
            .filter(|i: &StoreItem| language.map(|l: &str| i.language == l).unwrap_or(true))
            .collect()
    }

    fn items_interlinear(&self, language: Option<&str>) -> Vec<StoreItem> {
        self.translations
            .iter()
            .filter(|t: &&GenericCatalogBible| {
                t.has_strongs.unwrap_or(false) || t.id == "BSB" || t.id.starts_with("eng_kjv")
            })
            .filter(|t: &&GenericCatalogBible| {
                language.map(|l: &str| t.language_code == l).unwrap_or(true)
            })
            .map(Self::translation_item)
            .collect()
    }

    fn items_public_domain(&self, language: Option<&str>) -> Vec<StoreItem> {
        const PUBLIC_DOMAIN: &[&str] = &["eng_kjv", "eng_web", "eng_asv", "eng_kja", "BSB"];
        self.translations
            .iter()
            .filter(|t: &&GenericCatalogBible| PUBLIC_DOMAIN.contains(&t.id.as_str()))
            .filter(|t: &&GenericCatalogBible| {
                language.map(|l: &str| t.language_code == l).unwrap_or(true)
            })
            .map(Self::translation_item)
            .collect()
    }

    fn items_commentaries(&self, language: Option<&str>) -> Vec<StoreItem> {
        self.commentaries
            .iter()
            .filter(|c: &&HelloCommentaryMeta| {
                language.map(|l: &str| c.language == l).unwrap_or(true)
            })
            .map(Self::commentary_item)
            .collect()
    }

    fn items_datasets(&self, language: Option<&str>) -> Vec<StoreItem> {
        self.datasets
            .iter()
            .filter(|d: &&HelloDatasetMeta| d.id != "theographic")
            .filter(|d: &&HelloDatasetMeta| language.map(|l: &str| d.language == l).unwrap_or(true))
            .map(Self::dataset_item)
            .collect()
    }

    fn items_all(&self, language: Option<&str>) -> Vec<StoreItem> {
        let mut items: Vec<StoreItem> = Vec::new();
        items.extend(
            self.translations
                .iter()
                .filter(|t: &&GenericCatalogBible| {
                    language.map(|l: &str| t.language_code == l).unwrap_or(true)
                })
                .map(Self::translation_item),
        );
        items.extend(
            self.commentaries
                .iter()
                .filter(|c: &&HelloCommentaryMeta| {
                    language.map(|l: &str| c.language == l).unwrap_or(true)
                })
                .map(Self::commentary_item),
        );
        items.extend(
            self.datasets
                .iter()
                .filter(|d: &&HelloDatasetMeta| {
                    language.map(|l: &str| d.language == l).unwrap_or(true)
                })
                .map(Self::dataset_item),
        );
        items.sort_by(|a: &StoreItem, b: &StoreItem| a.name.cmp(&b.name));
        items
    }

    fn items_entities(&self, language: Option<&str>) -> Vec<StoreItem> {
        if language.map(|l: &str| l != "eng").unwrap_or(false) {
            return Vec::new();
        }
        vec![
            entity_set_item(
                "theographic:person",
                "Bible Characters",
                "Characters",
                "3067 people from Scripture",
                3067,
                "People",
            ),
            entity_set_item(
                "theographic:event",
                "Bible Events",
                "Events",
                "450 events from Scripture",
                450,
                "Events",
            ),
            entity_set_item(
                "theographic:place",
                "Bible Places",
                "Places",
                "1274 places from Scripture",
                1274,
                "Places",
            ),
            entity_set_item(
                "theographic:group",
                "People Groups",
                "Groups",
                "23 people groups from Scripture",
                23,
                "Groups",
            ),
        ]
    }

    // ── Search, Facets, Item Lookup & Source Resolution ──────────────────────

    pub fn search(&self, query: &str) -> Vec<StoreItem> {
        let q: String = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut items: Vec<StoreItem> = Vec::new();
        items.extend(
            self.translations
                .iter()
                .filter(|t: &&GenericCatalogBible| {
                    t.name.to_lowercase().contains(&q)
                        || t.english_name
                            .as_deref()
                            .map(|n: &str| n.to_lowercase().contains(&q))
                            .unwrap_or(false)
                        || t.short_name
                            .as_deref()
                            .map(|n: &str| n.to_lowercase().contains(&q))
                            .unwrap_or(false)
                        || t.language_name
                            .as_deref()
                            .map(|n: &str| n.to_lowercase().contains(&q))
                            .unwrap_or(false)
                })
                .map(Self::translation_item),
        );
        items.extend(
            self.commentaries
                .iter()
                .filter(|c: &&HelloCommentaryMeta| {
                    c.name.to_lowercase().contains(&q) || c.english_name.to_lowercase().contains(&q)
                })
                .map(Self::commentary_item),
        );
        items.extend(
            self.datasets
                .iter()
                .filter(|d: &&HelloDatasetMeta| {
                    d.name.to_lowercase().contains(&q) || d.english_name.to_lowercase().contains(&q)
                })
                .map(Self::dataset_item),
        );
        items.sort_by(|a: &StoreItem, b: &StoreItem| a.name.cmp(&b.name));
        items
    }

    pub fn languages(&self) -> Vec<LanguageFacet> {
        self.languages.clone()
    }

    pub fn item(&self, id: &str, kind: &str) -> Option<StoreItem> {
        match kind {
            "translation" => self
                .translations
                .iter()
                .find(|t: &&GenericCatalogBible| t.id == id)
                .map(Self::translation_item),
            "commentary" => self
                .commentaries
                .iter()
                .find(|c: &&HelloCommentaryMeta| c.id == id)
                .map(Self::commentary_item),
            "dataset" => self
                .datasets
                .iter()
                .find(|d: &&HelloDatasetMeta| d.id == id)
                .map(Self::dataset_item),
            "entity_set" => match id {
                "theographic:person" => Some(entity_set_item(
                    "theographic:person",
                    "Bible Characters",
                    "Characters",
                    "3067 people from Scripture",
                    3067,
                    "People",
                )),
                "theographic:event" => Some(entity_set_item(
                    "theographic:event",
                    "Bible Events",
                    "Events",
                    "450 events from Scripture",
                    450,
                    "Events",
                )),
                "theographic:place" => Some(entity_set_item(
                    "theographic:place",
                    "Bible Places",
                    "Places",
                    "1274 places from Scripture",
                    1274,
                    "Places",
                )),
                "theographic:group" => Some(entity_set_item(
                    "theographic:group",
                    "People Groups",
                    "Groups",
                    "23 people groups from Scripture",
                    23,
                    "Groups",
                )),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn source_for_translation(&self, translation_id: &str) -> Option<String> {
        self.translations
            .iter()
            .find(|t: &&GenericCatalogBible| t.id == translation_id)
            .map(|t: &GenericCatalogBible| t.source_id.clone())
    }

    // ── Item Builders ─────────────────────────────────────────────────────────

    fn translation_item(t: &GenericCatalogBible) -> StoreItem {
        let has_strongs: bool = t
            .has_strongs
            .unwrap_or_else(|| t.id == "BSB" || t.id.starts_with("eng_kjv"));
        let badge: Option<String> = match t.id.as_str() {
            "BSB" => Some("Interlinear".to_string()),
            "eng_kjv" | "eng_kja" | "eng_web" | "eng_asv" => Some("Public domain".to_string()),
            _ => None,
        };
        let lang_label: String = t
            .language_english_name
            .clone()
            .unwrap_or_else(|| t.language_code.clone());
        let books: i32 = t.book_count.unwrap_or(0);
        let chapters: i32 = t.chapter_count.unwrap_or(0);
        let verses: i32 = t.verse_count.unwrap_or(0);

        StoreItem {
            id: t.id.clone(),
            source_id: t.source_id.clone(),
            kind: "translation".to_string(),
            name: t.name.clone(),
            english_name: t.english_name.clone().unwrap_or_else(|| t.name.clone()),
            short_name: t.short_name.clone().unwrap_or_else(|| t.id.clone()),
            language: t.language_code.clone(),
            language_name: t.language_name.clone(),
            description: format!("{} • {} books • {} verses", lang_label, books, verses),
            badge,
            installed: false,
            has_strongs,
            has_audio: t.has_audio,
            book_count: books,
            chapter_count: chapters,
            verse_count: verses,
        }
    }

    fn commentary_item(c: &HelloCommentaryMeta) -> StoreItem {
        StoreItem {
            id: c.id.clone(),
            source_id: "helloao".to_string(),
            kind: "commentary".to_string(),
            name: c.name.clone(),
            english_name: c.english_name.clone(),
            short_name: c.name.clone(),
            language: c.language.clone(),
            language_name: None,
            description: format!(
                "Commentary • {} books • {} chapters",
                c.number_of_books, c.total_number_of_chapters
            ),
            badge: Some("Commentary".to_string()),
            installed: false,
            has_strongs: false,
            has_audio: false,
            book_count: c.number_of_books,
            chapter_count: c.total_number_of_chapters,
            verse_count: c.total_number_of_verses,
        }
    }

    fn dataset_item(d: &HelloDatasetMeta) -> StoreItem {
        let badge: &str = match d.id.as_str() {
            "stepbible-lexicon" => "Lexicon",
            "bsb-interlinear" => "Interlinear",
            _ => "Dataset",
        };
        StoreItem {
            id: d.id.clone(),
            source_id: "helloao".to_string(),
            kind: "dataset".to_string(),
            name: d.name.clone(),
            english_name: d.english_name.clone(),
            short_name: d.name.clone(),
            language: d.language.clone(),
            language_name: None,
            description: format!("Study dataset • {} books", d.total_number_of_books),
            badge: Some(badge.to_string()),
            installed: false,
            has_strongs: d.id == "stepbible-lexicon",
            has_audio: false,
            book_count: d.total_number_of_books,
            chapter_count: 0,
            verse_count: 0,
        }
    }

    fn build_facets(
        translations: &[GenericCatalogBible],
        commentaries: &[HelloCommentaryMeta],
    ) -> Vec<LanguageFacet> {
        let mut map: HashMap<String, (String, String, i32, i32)> = HashMap::new();
        for t in translations {
            let name: String = t
                .language_name
                .clone()
                .unwrap_or_else(|| t.language_code.clone());
            let eng: String = t
                .language_english_name
                .clone()
                .unwrap_or_else(|| name.clone());
            let entry: &mut (String, String, i32, i32) = map
                .entry(t.language_code.clone())
                .or_insert((name, eng, 0, 0));
            entry.2 += 1;
        }
        for c in commentaries {
            let entry: &mut (String, String, i32, i32) = map.entry(c.language.clone()).or_insert((
                c.language.clone(),
                c.language.clone(),
                0,
                0,
            ));
            entry.3 += 1;
        }
        let mut out: Vec<LanguageFacet> = map
            .into_iter()
            .map(
                |(code, (name, eng, tc, cc)): (String, (String, String, i32, i32))| -> LanguageFacet {
                    LanguageFacet {
                        code,
                        name,
                        english_name: eng,
                        translation_count: tc,
                        commentary_count: cc,
                    }
                },
            )
            .collect();
        out.sort_by(|a: &LanguageFacet, b: &LanguageFacet| a.english_name.cmp(&b.english_name));
        out
    }
}

fn entity_set_item(
    id: &str,
    name: &str,
    short_name: &str,
    description: &str,
    count: i32,
    badge: &str,
) -> StoreItem {
    StoreItem {
        id: id.to_string(),
        source_id: "helloao".to_string(),
        kind: "entity_set".to_string(),
        name: name.to_string(),
        english_name: name.to_string(),
        short_name: short_name.to_string(),
        language: "eng".to_string(),
        language_name: Some("English".to_string()),
        description: description.to_string(),
        badge: Some(badge.to_string()),
        installed: false,
        has_strongs: false,
        has_audio: false,
        book_count: count,
        chapter_count: 0,
        verse_count: 0,
    }
}
