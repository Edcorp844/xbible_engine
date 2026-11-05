use crate::db::catalog;

use super::client::{
    BibleClient, FreeBibleCommentary, FreeBibleDataset, FreeBibleTranslation,
};
use log::error;
use std::collections::HashMap;

#[derive(Debug)]
pub struct Store {
    translations: Vec<FreeBibleTranslation>,
    commentaries: Vec<FreeBibleCommentary>,
    datasets: Vec<FreeBibleDataset>,
    languages: Vec<LanguageFacet>,
    translation_languages: HashMap<String, String>,
    commentary_languages: HashMap<String, String>,
    dataset_languages: HashMap<String, String>,
}

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

impl Store {
    pub fn fetch(
        client: &BibleClient,
        conn: Option<&rusqlite::Connection>,
    ) -> Result<Self, String> {
        match (client.translations(), client.commentaries(), client.datasets()) {
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
                Ok(Self::from_parts(t, c, d))
            }
            _ => {
                let conn = conn.ok_or("network failed and no local database")?;
                let t = catalog::load_translations(conn).map_err(|e| e.to_string())?;
                let c = catalog::load_commentaries(conn).map_err(|e| e.to_string())?;
                let d = catalog::load_datasets(conn).map_err(|e| e.to_string())?;
                if t.is_empty() {
                    return Err("network failed and local database is empty".into());
                }
                Ok(Self::from_parts(t, c, d))
            }
        }
    }

    /// Build a Store from already-fetched parts. Public so callers that
    /// fetched data themselves (e.g. `refresh` in `store_api`) can skip
    /// `fetch` and its lock-holding behavior.
    pub fn from_parts(
        translations: Vec<FreeBibleTranslation>,
        commentaries: Vec<FreeBibleCommentary>,
        datasets: Vec<FreeBibleDataset>,
    ) -> Self {
        let languages = Self::build_facets(&translations, &commentaries);
        let translation_languages = translations
            .iter()
            .map(|t| (t.id.clone(), t.language.clone()))
            .collect();
        let commentary_languages = commentaries
            .iter()
            .map(|c| (c.id.clone(), c.language.clone()))
            .collect();
        let dataset_languages = datasets
            .iter()
            .map(|d| (d.id.clone(), d.language.clone()))
            .collect();
        Self {
            translations,
            commentaries,
            datasets,
            languages,
            translation_languages,
            commentary_languages,
            dataset_languages,
        }
    }

    // ── Feed ─────────────────────────────────────────────────────

    pub fn home_feed(&self, language: Option<&str>) -> Vec<StoreShelf> {
        vec![
            self.shelf_featured(language),
            self.shelf_interlinear(language),
            self.shelf_public_domain(language),
            self.shelf_commentaries(language),
            self.shelf_datasets(language),
            self.shelf_all(language),
        ]
        .into_iter()
        .filter(|s| !s.items.is_empty())
        .collect()
    }

    fn shelf_featured(&self, language: Option<&str>) -> StoreShelf {
        let ids: &[(&str, &str)] = &[
            ("BSB", "translation"),
            ("eng_kjv", "translation"),
            ("eng_web", "translation"),
            ("adam-clarke", "commentary"),
            ("tyndale", "commentary"),
            ("stepbible-lexicon", "dataset"),
        ];
        let items: Vec<StoreItem> = ids
            .iter()
            .filter_map(|(id, kind)| self.item(id, kind))
            .filter(|i| language.map(|l| i.language == l).unwrap_or(true))
            .collect();
        StoreShelf {
            title: "Featured".into(),
            subtitle: Some("Hand-picked for study".into()),
            items,
        }
    }

    fn shelf_interlinear(&self, language: Option<&str>) -> StoreShelf {
        let items = self
            .translations
            .iter()
            .filter(|t| t.id == "BSB" || t.id.starts_with("eng_kjv"))
            .filter(|t| language.map(|l| t.language == l).unwrap_or(true))
            .map(Self::translation_to_item)
            .collect();
        StoreShelf {
            title: "Word Study".into(),
            subtitle: Some("Original-language alignment with Strong's".into()),
            items,
        }
    }

    fn shelf_public_domain(&self, language: Option<&str>) -> StoreShelf {
        let public_domain: &[&str] = &["eng_kjv", "eng_web", "eng_asv", "eng_kja", "BSB"];
        let items = self
            .translations
            .iter()
            .filter(|t| public_domain.contains(&t.id.as_str()))
            .filter(|t| language.map(|l| t.language == l).unwrap_or(true))
            .map(Self::translation_to_item)
            .collect();
        StoreShelf {
            title: "Public Domain".into(),
            subtitle: Some("Free to use, no strings attached".into()),
            items,
        }
    }

    fn shelf_commentaries(&self, language: Option<&str>) -> StoreShelf {
        let items = self
            .commentaries
            .iter()
            .filter(|c| language.map(|l| c.language == l).unwrap_or(true))
            .map(Self::commentary_to_item)
            .collect();
        StoreShelf {
            title: "Commentaries".into(),
            subtitle: Some("Verse-by-verse explanations and insights".into()),
            items,
        }
    }

    fn shelf_datasets(&self, language: Option<&str>) -> StoreShelf {
        let items = self
            .datasets
            .iter()
            .filter(|d| language.map(|l| d.language == l).unwrap_or(true))
            .map(Self::dataset_to_item)
            .collect();
        StoreShelf {
            title: "Study Tools".into(),
            subtitle: Some("Lexicons, cross-references, and more".into()),
            items,
        }
    }

    fn shelf_all(&self, language: Option<&str>) -> StoreShelf {
        let mut items: Vec<StoreItem> = Vec::new();
        items.extend(
            self.translations
                .iter()
                .filter(|t| language.map(|l| t.language == l).unwrap_or(true))
                .map(Self::translation_to_item),
        );
        items.extend(
            self.commentaries
                .iter()
                .filter(|c| language.map(|l| c.language == l).unwrap_or(true))
                .map(Self::commentary_to_item),
        );
        items.extend(
            self.datasets
                .iter()
                .filter(|d| language.map(|l| d.language == l).unwrap_or(true))
                .map(Self::dataset_to_item),
        );
        items.sort_by(|a, b| a.name.cmp(&b.name));
        StoreShelf {
            title: "All Resources".into(),
            subtitle: Some(format!("{} items", items.len())),
            items,
        }
    }

    // ── Search ───────────────────────────────────────────────────

    pub fn search(&self, query: &str) -> Vec<StoreItem> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let q = query.to_lowercase();
        let mut items: Vec<StoreItem> = Vec::new();
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
                .map(Self::translation_to_item),
        );
        items.extend(
            self.commentaries
                .iter()
                .filter(|c| {
                    c.name.to_lowercase().contains(&q) || c.english_name.to_lowercase().contains(&q)
                })
                .map(Self::commentary_to_item),
        );
        items.extend(
            self.datasets
                .iter()
                .filter(|d| {
                    d.name.to_lowercase().contains(&q) || d.english_name.to_lowercase().contains(&q)
                })
                .map(Self::dataset_to_item),
        );
        items.sort_by(|a, b| a.name.cmp(&b.name));
        items
    }

    // ── Facets ───────────────────────────────────────────────────

    pub fn languages(&self) -> Vec<LanguageFacet> {
        self.languages.clone()
    }

    // ── Item lookup ──────────────────────────────────────────────

    pub fn item(&self, id: &str, kind: &str) -> Option<StoreItem> {
        match kind {
            "translation" => self
                .translations
                .iter()
                .find(|t| t.id == id)
                .map(Self::translation_to_item),
            "commentary" => self
                .commentaries
                .iter()
                .find(|c| c.id == id)
                .map(Self::commentary_to_item),
            "dataset" => self
                .datasets
                .iter()
                .find(|d| d.id == id)
                .map(Self::dataset_to_item),
            _ => None,
        }
    }

    // ── Conversions ──────────────────────────────────────────────

    fn translation_to_item(t: &FreeBibleTranslation) -> StoreItem {
        let has_strongs = t.id == "BSB" || t.id.starts_with("eng_kjv");
        let badge = match t.id.as_str() {
            "BSB" => Some("Interlinear".into()),
            "eng_kjv" | "eng_kja" | "eng_web" | "eng_asv" => Some("Public domain".into()),
            _ => None,
        };
        StoreItem {
            id: t.id.clone(),
            kind: "translation".into(),
            name: t.name.clone(),
            english_name: t.english_name.clone(),
            short_name: t.short_name.clone(),
            language: t.language.clone(),
            language_name: t.language_name.clone(),
            description: format!(
                "{} • {} books • {} verses",
                t.language_english_name
                    .clone()
                    .unwrap_or_else(|| t.language.clone()),
                t.number_of_books,
                t.total_number_of_verses
            ),
            badge,
            installed: false,
            has_strongs,
            book_count: t.number_of_books,
            chapter_count: t.total_number_of_chapters,
            verse_count: t.total_number_of_verses,
        }
    }

    fn commentary_to_item(c: &FreeBibleCommentary) -> StoreItem {
        StoreItem {
            id: c.id.clone(),
            kind: "commentary".into(),
            name: c.name.clone(),
            english_name: c.english_name.clone(),
            short_name: c.name.clone(),
            language: c.language.clone(),
            language_name: None,
            description: format!(
                "Commentary • {} books • {} chapters",
                c.number_of_books, c.total_number_of_chapters
            ),
            badge: Some("Commentary".into()),
            installed: false,
            has_strongs: false,
            book_count: c.number_of_books,
            chapter_count: c.total_number_of_chapters,
            verse_count: c.total_number_of_verses,
        }
    }

    fn dataset_to_item(d: &FreeBibleDataset) -> StoreItem {
        let badge = match d.id.as_str() {
            "stepbible-lexicon" => Some("Lexicon".into()),
            "bsb-interlinear" => Some("Interlinear".into()),
            _ => Some("Dataset".into()),
        };
        StoreItem {
            id: d.id.clone(),
            kind: "dataset".into(),
            name: d.name.clone(),
            english_name: d.english_name.clone(),
            short_name: d.name.clone(),
            language: d.language.clone(),
            language_name: None,
            description: format!("Study dataset • {} books", d.total_number_of_books),
            badge,
            installed: false,
            has_strongs: d.id == "stepbible-lexicon",
            book_count: d.total_number_of_books,
            chapter_count: 0,
            verse_count: 0,
        }
    }

    // ── Facet builder ────────────────────────────────────────────

    fn build_facets(
        translations: &[FreeBibleTranslation],
        commentaries: &[FreeBibleCommentary],
    ) -> Vec<LanguageFacet> {
        let mut map: HashMap<String, (String, String, i32, i32)> = HashMap::new();
        for t in translations {
            let name = t
                .language_name
                .clone()
                .unwrap_or_else(|| t.language.clone());
            let eng = t
                .language_english_name
                .clone()
                .unwrap_or_else(|| name.clone());
            let e = map.entry(t.language.clone()).or_insert((name, eng, 0, 0));
            e.2 += 1;
        }
        for c in commentaries {
            let e = map.entry(c.language.clone()).or_insert((
                c.language.clone(),
                c.language.clone(),
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