// freebible_engine/store.rs

use std::collections::HashMap;

use log::error;

use super::catalog::{self, DomainCommentary, DomainDataset, DomainTranslation};
use super::client::{BibleClient, FreeBibleCommentary, FreeBibleDataset, FreeBibleTranslation};
use super::values::{
    BookCount, ChapterCount, Flag, LanguageCode, ShelfId, StoreItemId, StoreItemKind, VerseCount,
};

// ── UniFFI records (public wire format — primitives only) ───────

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

// ── Domain item (used internally, newtypes everywhere) ──────────

#[derive(Debug, Clone)]
pub struct DomainStoreItem {
    pub id: StoreItemId,
    pub name: String,
    pub english_name: String,
    pub short_name: String,
    pub language: LanguageCode,
    pub language_name: Option<String>,
    pub description: String,
    pub badge: Option<ItemBadge>,
    pub installed: Flag,
    pub has_strongs: Flag,
    pub books: BookCount,
    pub chapters: ChapterCount,
    pub verses: VerseCount,
}

/// Closed set of badge strings the UI may show. Adding a new one
/// forces updating `as_str` and `Display`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemBadge {
    Interlinear,
    PublicDomain,
    Commentary,
    Lexicon,
    Dataset,
    People,
    Events,
    Places,
    Groups,
}

impl ItemBadge {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Interlinear => "Interlinear",
            Self::PublicDomain => "Public domain",
            Self::Commentary => "Commentary",
            Self::Lexicon => "Lexicon",
            Self::Dataset => "Dataset",
            Self::People => "People",
            Self::Events => "Events",
            Self::Places => "Places",
            Self::Groups => "Groups",
        }
    }
}

// ── FFI conversions ────────────────────────────────────────────

impl From<DomainStoreItem> for StoreItem {
    fn from(d: DomainStoreItem) -> Self {
        Self {
            id: d.id.raw,
            kind: d.id.kind.as_wire().to_string(),
            name: d.name,
            english_name: d.english_name,
            short_name: d.short_name,
            language: d.language.into_inner(),
            language_name: d.language_name,
            description: d.description,
            badge: d.badge.map(|b| b.as_str().to_string()),
            installed: d.installed.as_bool(),
            has_strongs: d.has_strongs.as_bool(),
            book_count: d.books.raw(),
            chapter_count: d.chapters.raw(),
            verse_count: d.verses.raw(),
        }
    }
}

/// Try to parse a `StoreItem`'s wire fields back into the domain
/// type. Used only by the FFI facade when it needs to re-key items.
pub fn domain_kind_of(item: &StoreItem) -> Option<StoreItemKind> {
    StoreItemKind::from_wire(&item.kind)
}

// ── The Store itself ───────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Store {
    translations: Vec<DomainTranslation>,
    commentaries: Vec<DomainCommentary>,
    datasets: Vec<DomainDataset>,
    languages: Vec<LanguageFacet>,
}

// ── Fetch ──────────────────────────────────────────────────────

impl Store {
    pub fn fetch_index(
        client: &BibleClient,
        conn: Option<&rusqlite::Connection>,
    ) -> Result<Self, String> {
        match (
            client.translations(),
            client.commentaries(),
            client.datasets(),
        ) {
            (Ok(t), Ok(c), Ok(d)) => {
                if let Some(conn) = conn {
                    if let Err(e) = catalog::save_translations(conn, &t) {
                        error!("save_translations failed: {}", e);
                    }
                    if let Err(e) = catalog::save_commentaries(conn, &c) {
                        error!("save_commentaries failed: {}", e);
                    }
                    if let Err(e) = catalog::save_datasets(conn, &d) {
                        error!("save_datasets failed: {}", e);
                    }
                }
                Ok(Self::from_client_parts(t, c, d))
            }
            _ => {
                let conn = conn.ok_or("network failed and no local database")?;
                let t = catalog::load_translations(conn).map_err(|e| e.to_string())?;
                let c = catalog::load_commentaries(conn).map_err(|e| e.to_string())?;
                let d = catalog::load_datasets(conn).map_err(|e| e.to_string())?;
                if t.is_empty() {
                    return Err("network failed and local database is empty".into());
                }
                Ok(Self::from_domains(t, c, d))
            }
        }
    }

    /// Convert freshly-fetched client records into the domain model.
    /// Relies on `From<&FreeBibleTranslation>` etc. defined in
    /// `freebible_engine::catalog` so the conversion lives in exactly one place.
    fn from_client_parts(
        translations: Vec<FreeBibleTranslation>,
        commentaries: Vec<FreeBibleCommentary>,
        datasets: Vec<FreeBibleDataset>,
    ) -> Self {
        let t: Vec<DomainTranslation> = translations.iter().map(Into::into).collect();
        let c: Vec<DomainCommentary> = commentaries.iter().map(Into::into).collect();
        let d: Vec<DomainDataset> = datasets.iter().map(Into::into).collect();
        Self::from_domains(t, c, d)
    }

    fn from_domains(
        translations: Vec<DomainTranslation>,
        commentaries: Vec<DomainCommentary>,
        datasets: Vec<DomainDataset>,
    ) -> Self {
        let languages = Self::build_facets(&translations, &commentaries);
        Self {
            translations,
            commentaries,
            datasets,
            languages,
        }
    }

    // ── Feed ────────────────────────────────────────────────────

    pub fn home_feed(&self, language: Option<&str>) -> Vec<StoreShelf> {
        ShelfId::ALL
            .iter()
            .map(|&shelf| self.shelf(shelf, language))
            .filter(|s| !s.items.is_empty())
            .collect()
    }

    pub fn shelf(&self, shelf: ShelfId, language: Option<&str>) -> StoreShelf {
        let items = self.shelf_items(shelf, language);
        StoreShelf {
            title: shelf.title().to_string(),
            subtitle: shelf.subtitle().map(|s| s.to_string()).or_else(|| {
                if shelf == ShelfId::AllResources {
                    Some(format!("{} items", items.len()))
                } else {
                    None
                }
            }),
            items: items.into_iter().map(StoreItem::from).collect(),
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
        let all = self.shelf_items(shelf, language);
        all.into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(StoreItem::from)
            .collect()
    }

    pub fn shelf_total(&self, shelf_id: &str, language: Option<&str>) -> u32 {
        let Some(shelf) = ShelfId::from_title(shelf_id) else {
            return 0;
        };
        self.shelf_items(shelf, language).len() as u32
    }

    fn shelf_items(&self, shelf: ShelfId, language: Option<&str>) -> Vec<DomainStoreItem> {
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

    fn items_featured(&self, language: Option<&str>) -> Vec<DomainStoreItem> {
        let featured_ids: &[StoreItemId] = &[
            StoreItemId::translation("BSB"),
            StoreItemId::translation("eng_kjv"),
            StoreItemId::translation("eng_web"),
            StoreItemId::commentary("adam-clarke"),
            StoreItemId::commentary("tyndale"),
            StoreItemId::dataset("stepbible-lexicon"),
        ];
        featured_ids
            .iter()
            .filter_map(|id| self.item(id))
            .filter(|i| language.map(|l| i.language.as_str() == l).unwrap_or(true))
            .collect()
    }

    fn items_interlinear(&self, language: Option<&str>) -> Vec<DomainStoreItem> {
        self.translations
            .iter()
            .filter(|t| t.id == "BSB" || t.id.starts_with("eng_kjv"))
            .filter(|t| language.map(|l| t.language.as_str() == l).unwrap_or(true))
            .map(Self::translation_item)
            .collect()
    }

    fn items_public_domain(&self, language: Option<&str>) -> Vec<DomainStoreItem> {
        const PUBLIC_DOMAIN: &[&str] = &["eng_kjv", "eng_web", "eng_asv", "eng_kja", "BSB"];
        self.translations
            .iter()
            .filter(|t| PUBLIC_DOMAIN.contains(&t.id.as_str()))
            .filter(|t| language.map(|l| t.language.as_str() == l).unwrap_or(true))
            .map(Self::translation_item)
            .collect()
    }

    fn items_commentaries(&self, language: Option<&str>) -> Vec<DomainStoreItem> {
        self.commentaries
            .iter()
            .filter(|c| language.map(|l| c.language.as_str() == l).unwrap_or(true))
            .map(Self::commentary_item)
            .collect()
    }

    fn items_datasets(&self, language: Option<&str>) -> Vec<DomainStoreItem> {
        self.datasets
            .iter()
            .filter(|d| d.id != "theographic")
            .filter(|d| language.map(|l| d.language.as_str() == l).unwrap_or(true))
            .map(Self::dataset_item)
            .collect()
    }

    fn items_all(&self, language: Option<&str>) -> Vec<DomainStoreItem> {
        let mut items = Vec::new();
        items.extend(
            self.translations
                .iter()
                .filter(|t| language.map(|l| t.language.as_str() == l).unwrap_or(true))
                .map(Self::translation_item),
        );
        items.extend(
            self.commentaries
                .iter()
                .filter(|c| language.map(|l| c.language.as_str() == l).unwrap_or(true))
                .map(Self::commentary_item),
        );
        items.extend(
            self.datasets
                .iter()
                .filter(|d| language.map(|l| d.language.as_str() == l).unwrap_or(true))
                .map(Self::dataset_item),
        );
        items.sort_by(|a, b| a.name.cmp(&b.name));
        items
    }

    fn items_entities(&self, language: Option<&str>) -> Vec<DomainStoreItem> {
        if language.map(|l| l != "eng").unwrap_or(false) {
            return Vec::new();
        }
        vec![
            entity_set_item(
                "theographic:person",
                "Bible Characters",
                "Characters",
                "3067 people from Scripture",
                3067,
                ItemBadge::People,
            ),
            entity_set_item(
                "theographic:event",
                "Bible Events",
                "Events",
                "450 events from Scripture",
                450,
                ItemBadge::Events,
            ),
            entity_set_item(
                "theographic:place",
                "Bible Places",
                "Places",
                "1274 places from Scripture",
                1274,
                ItemBadge::Places,
            ),
            entity_set_item(
                "theographic:group",
                "People Groups",
                "Groups",
                "23 people groups from Scripture",
                23,
                ItemBadge::Groups,
            ),
        ]
    }

    // ── Search ──────────────────────────────────────────────────

    pub fn search(&self, query: &str) -> Vec<StoreItem> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut items = Vec::new();
        items.extend(
            self.translations
                .iter()
                .filter(|t| {
                    t.name.to_lowercase().contains(&q)
                        || t.english_name.to_lowercase().contains(&q)
                        || t.short_name.to_lowercase().contains(&q)
                        || t.language_name
                            .as_deref()
                            .map(|n| n.to_lowercase().contains(&q))
                            .unwrap_or(false)
                })
                .map(Self::translation_item),
        );
        items.extend(
            self.commentaries
                .iter()
                .filter(|c| {
                    c.name.to_lowercase().contains(&q)
                        || c.english_name.to_lowercase().contains(&q)
                })
                .map(Self::commentary_item),
        );
        items.extend(
            self.datasets
                .iter()
                .filter(|d| {
                    d.name.to_lowercase().contains(&q)
                        || d.english_name.to_lowercase().contains(&q)
                })
                .map(Self::dataset_item),
        );
        items.sort_by(|a, b| a.name.cmp(&b.name));
        items.into_iter().map(StoreItem::from).collect()
    }

    // ── Facets ──────────────────────────────────────────────────

    pub fn languages(&self) -> Vec<LanguageFacet> {
        self.languages.clone()
    }

    // ── Item lookup ────────────────────────────────────────────

    pub fn item(&self, id: &StoreItemId) -> Option<DomainStoreItem> {
        match id.kind {
            StoreItemKind::Translation => self
                .translations
                .iter()
                .find(|t| t.id == id.raw)
                .map(Self::translation_item),
            StoreItemKind::Commentary => self
                .commentaries
                .iter()
                .find(|c| c.id == id.raw)
                .map(Self::commentary_item),
            StoreItemKind::Dataset => self
                .datasets
                .iter()
                .find(|d| d.id == id.raw)
                .map(Self::dataset_item),
            StoreItemKind::EntitySet => {
                let parts: Vec<&str> = id.raw.splitn(2, ':').collect();
                match parts.as_slice() {
                    ["theographic", "person"] => Some(entity_set_item(
                        "theographic:person",
                        "Bible Characters",
                        "Characters",
                        "3067 people from Scripture",
                        3067,
                        ItemBadge::People,
                    )),
                    ["theographic", "event"] => Some(entity_set_item(
                        "theographic:event",
                        "Bible Events",
                        "Events",
                        "450 events from Scripture",
                        450,
                        ItemBadge::Events,
                    )),
                    ["theographic", "place"] => Some(entity_set_item(
                        "theographic:place",
                        "Bible Places",
                        "Places",
                        "1274 places from Scripture",
                        1274,
                        ItemBadge::Places,
                    )),
                    ["theographic", "group"] => Some(entity_set_item(
                        "theographic:group",
                        "People Groups",
                        "Groups",
                        "23 people groups from Scripture",
                        23,
                        ItemBadge::Groups,
                    )),
                    _ => None,
                }
            }
        }
    }

    // ── Domain item builders ──────────────────────────────────

    fn translation_item(t: &DomainTranslation) -> DomainStoreItem {
        let has_strongs = matches!(t.id.as_str(), "BSB") || t.id.starts_with("eng_kjv");
        let badge = match t.id.as_str() {
            "BSB" => Some(ItemBadge::Interlinear),
            "eng_kjv" | "eng_kja" | "eng_web" | "eng_asv" => Some(ItemBadge::PublicDomain),
            _ => None,
        };
        DomainStoreItem {
            id: StoreItemId::translation(&t.id),
            name: t.name.clone(),
            english_name: t.english_name.clone(),
            short_name: t.short_name.clone(),
            language: t.language.clone(),
            language_name: t.language_name.clone(),
            description: format!(
                "{} • {} books • {} verses",
                t.language_english_name
                    .clone()
                    .unwrap_or_else(|| t.language.as_str().to_string()),
                t.books,
                t.verses
            ),
            badge,
            installed: Flag::Off,
            has_strongs: Flag::from_bool(has_strongs),
            books: t.books,
            chapters: t.chapters,
            verses: t.verses,
        }
    }

    fn commentary_item(c: &DomainCommentary) -> DomainStoreItem {
        DomainStoreItem {
            id: StoreItemId::commentary(&c.id),
            name: c.name.clone(),
            english_name: c.english_name.clone(),
            short_name: c.name.clone(),
            language: c.language.clone(),
            language_name: None,
            description: format!("Commentary • {} books • {} chapters", c.books, c.chapters),
            badge: Some(ItemBadge::Commentary),
            installed: Flag::Off,
            has_strongs: Flag::Off,
            books: c.books,
            chapters: c.chapters,
            verses: c.verses,
        }
    }

    fn dataset_item(d: &DomainDataset) -> DomainStoreItem {
        let badge = match d.id.as_str() {
            "stepbible-lexicon" => ItemBadge::Lexicon,
            "bsb-interlinear" => ItemBadge::Interlinear,
            _ => ItemBadge::Dataset,
        };
        DomainStoreItem {
            id: StoreItemId::dataset(&d.id),
            name: d.name.clone(),
            english_name: d.english_name.clone(),
            short_name: d.name.clone(),
            language: d.language.clone(),
            language_name: None,
            description: format!("Study dataset • {} books", d.books),
            badge: Some(badge),
            installed: Flag::Off,
            has_strongs: Flag::from_bool(d.id == "stepbible-lexicon"),
            books: d.books,
            chapters: ChapterCount::ZERO,
            verses: VerseCount::ZERO,
        }
    }

    // ── Facets ──────────────────────────────────────────────────

    fn build_facets(
        translations: &[DomainTranslation],
        commentaries: &[DomainCommentary],
    ) -> Vec<LanguageFacet> {
        let mut map: HashMap<String, (String, String, i32, i32)> = HashMap::new();
        for t in translations {
            let name = t
                .language_name
                .clone()
                .unwrap_or_else(|| t.language.as_str().to_string());
            let eng = t
                .language_english_name
                .clone()
                .unwrap_or_else(|| name.clone());
            let e = map
                .entry(t.language.as_str().to_string())
                .or_insert((name, eng, 0, 0));
            e.2 += 1;
        }
        for c in commentaries {
            let e = map.entry(c.language.as_str().to_string()).or_insert((
                c.language.as_str().to_string(),
                c.language.as_str().to_string(),
                0,
                0,
            ));
            e.3 += 1;
        }
        let mut out: Vec<LanguageFacet> = map
            .into_iter()
            .map(|(code, (name, eng, tc, cc))| LanguageFacet {
                code,
                name,
                english_name: eng,
                translation_count: tc,
                commentary_count: cc,
            })
            .collect();
        out.sort_by(|a, b| a.english_name.cmp(&b.english_name));
        out
    }
}

// ── Helpers ────────────────────────────────────────────────────

fn entity_set_item(
    id: &str,
    name: &str,
    short_name: &str,
    description: &str,
    count: i32,
    badge: ItemBadge,
) -> DomainStoreItem {
    DomainStoreItem {
        id: StoreItemId::entity_set(id),
        name: name.into(),
        english_name: name.into(),
        short_name: short_name.into(),
        language: LanguageCode("eng".into()),
        language_name: Some("English".into()),
        description: description.into(),
        badge: Some(badge),
        installed: Flag::Off,
        has_strongs: Flag::Off,
        books: BookCount::from_raw(count),
        chapters: ChapterCount::ZERO,
        verses: VerseCount::ZERO,
    }
}