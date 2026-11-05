use std::time::Duration;

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use uniffi;

// ── UniFFI records ────────────────────────────────────────────────

#[derive(Debug, Clone, uniffi::Record)]
pub struct FreeBibleTranslation {
    pub id: String,
    pub name: String,
    pub english_name: String,
    pub short_name: String,
    pub language: String,
    pub language_name: Option<String>,
    pub language_english_name: Option<String>,
    pub text_direction: String,
    pub number_of_books: i32,
    pub total_number_of_chapters: i32,
    pub total_number_of_verses: i32,
    pub license_url: Option<String>,
    pub license_notes: Option<String>,
    pub raw_json: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FreeBibleBook {
    pub id: String,
    pub name: String,
    pub number_of_chapters: i32,
    pub common_name: Option<String>,
    pub title: Option<String>,
    pub book_order: Option<i32>,
    pub audio_links_json: Option<String>,
    pub raw_json: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FreeBibleVerse {
    pub number: i32,
    pub text: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FreeBibleChapter {
    pub translation_id: String,
    pub book_id: String,
    pub chapter_number: i32,
    pub content: Vec<FreeBibleVerse>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FreeBibleChapterItem {
    pub position: i32,
    pub item_type: String,
    pub verse_number: Option<i32>,
    pub text: Option<String>,
    pub poem: bool,
    pub words_of_jesus: bool,
    pub footnote_ids: Vec<String>,
    pub raw_json: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FreeBibleFootnote {
    pub note_id: String,
    pub caller: Option<String>,
    pub text: String,
    pub reference: Option<String>,
    pub raw_json: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FreeBibleAudioTiming {
    pub verse: i32,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FreeBibleCommentary {
    pub id: String,
    pub name: String,
    pub english_name: String,
    pub language: String,
    pub license_url: Option<String>,
    pub license_notes: Option<String>,
    pub number_of_books: i32,
    pub total_number_of_chapters: i32,
    pub total_number_of_verses: i32,
    pub raw_json: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FreeBibleCommentaryChapter {
    pub commentary_id: String,
    pub book_id: String,
    pub chapter_number: i32,
    pub content: Vec<FreeBibleVerse>,
}

#[derive(Debug, Deserialize, Clone, uniffi::Record)]
pub struct CommentaryProfile {
    pub id: String,
    pub name: String,
    pub body: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FreeBibleDataset {
    pub id: String,
    pub name: String,
    pub english_name: String,
    pub language: String,
    pub license_url: Option<String>,
    pub license_notes: Option<String>,
    pub total_number_of_books: i32,
    pub raw_json: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FreeBibleLexiconEntry {
    pub strongs: String,
    pub lemma: Option<String>,
    pub transliteration: Option<String>,
    pub part_of_speech: Option<String>,
    pub gloss: Option<String>,
    pub definition: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FreeBibleComplete {
    pub translation_id: String,
    pub books: Vec<FreeBibleCompleteBook>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FreeBibleCompleteBook {
    pub id: String,
    pub name: String,
    pub number_of_chapters: i32,
    pub common_name: Option<String>,
    pub title: Option<String>,
    pub book_order: Option<i32>,
    pub audio_links_json: Option<String>,
    pub raw_json: String,
    pub chapters: Vec<FreeBibleCompleteChapter>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FreeBibleCompleteChapter {
    pub number: i32,
    pub verses: Vec<FreeBibleVerse>,
    pub items: Vec<FreeBibleChapterItem>,
    pub footnotes: Vec<FreeBibleFootnote>,
    pub audio_timings: Vec<FreeBibleAudioTiming>,
    pub raw_json: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct EntitySummary {
    pub id: String,
    pub name: String,
    /// people only — true if it's a proper name (e.g. "Aaron", not "the priest")
    pub is_proper_name: Option<bool>,
    /// people only — "Male" | "Female"
    pub gender: Option<String>,
    /// events only — the year the event started
    pub start_date: Option<String>,
    pub number_of_references: i32,
}

#[derive(Deserialize)]
struct RawEntitySummary {
    id: String,
    name: String,
    #[serde(rename = "isProperName", default)]
    is_proper_name: Option<bool>,
    #[serde(default)]
    gender: Option<String>,
    #[serde(rename = "startDate", default)]
    start_date: Option<String>,
    #[serde(rename = "numberOfReferences", default)]
    number_of_references: i32,
}

impl From<RawEntitySummary> for EntitySummary {
    fn from(r: RawEntitySummary) -> Self {
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
    people: Vec<RawEntitySummary>,
}

#[derive(Deserialize)]
struct EventsListResponse {
    events: Vec<RawEntitySummary>,
}

#[derive(Deserialize)]
struct PlacesListResponse {
    places: Vec<RawEntitySummary>,
}

#[derive(Deserialize)]
struct GroupsListResponse {
    groups: Vec<RawEntitySummary>,
}

// ── Client ────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct BibleClient {
    http: Client,
    base_url: String,
}

impl BibleClient {
    pub fn new() -> Self {
        #[cfg(target_os = "android")]
        let http = {
            use rustls::RootCertStore;

            // On Android, reqwest's default rustls backend goes through
            // `rustls-platform-verifier`, which requires a JNI handshake with an
            // Android Context. If that handshake hasn't happened, `.build()`
            // errors and later calls fail. Bypass it with a pre-built rustls
            // config using webpki roots instead.
            let mut roots = RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            let config = rustls::ClientConfig::builder()
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
                        "BibleClient: android preconfigured client build failed ({}), \
                     falling back to reqwest::blocking::Client::new()",
                        e
                    );
                    Client::new()
                }
            }
        };

        #[cfg(not(target_os = "android"))]
        let http = match Client::builder()
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
                    "BibleClient: client build failed ({}), \
                 falling back to reqwest::blocking::Client::new()",
                    e
                );
                Client::new()
            }
        };

        Self {
            http,
            base_url: "https://bible.helloao.org/api".to_string(),
        }
    }

    // ── Translations ──────────────────────────────────────────────

    pub fn translations(&self) -> Result<Vec<FreeBibleTranslation>, String> {
        let data: TranslationsResponse = self.get_json("available_translations.json")?;
        Ok(data
            .translations
            .into_iter()
            .map(|t| FreeBibleTranslation {
                id: t.id,
                name: t.name,
                english_name: t.english_name,
                short_name: t.short_name,
                language: t.language,
                language_name: t.language_name,
                language_english_name: t.language_english_name,
                text_direction: t.text_direction,
                number_of_books: t.number_of_books,
                total_number_of_chapters: t.total_number_of_chapters,
                total_number_of_verses: t.total_number_of_verses,
                license_url: t.license_url,
                license_notes: t.license_notes,
                raw_json: serde_json::to_string(&t.raw).unwrap_or_default(),
            })
            .collect())
    }

    pub fn books(&self, translation: &str) -> Result<Vec<FreeBibleBook>, String> {
        let data: BooksResponse = self.get_json(&format!("{}/books.json", translation))?;
        Ok(data
            .books
            .into_iter()
            .map(|b| FreeBibleBook {
                id: b.id,
                name: b.name,
                number_of_chapters: b.number_of_chapters,
                common_name: b.common_name,
                title: b.title,
                book_order: b.order,
                audio_links_json: b
                    .audio_links
                    .as_ref()
                    .and_then(|v| serde_json::to_string(v).ok()),
                raw_json: serde_json::to_string(&b.raw).ok(),
            })
            .collect())
    }

    pub fn chapter(
        &self,
        translation: &str,
        book: &str,
        chapter: u32,
    ) -> Result<FreeBibleChapter, String> {
        let data: ChapterResponse =
            self.get_json(&format!("{}/{}/{}.json", translation, book, chapter))?;
        Ok(FreeBibleChapter {
            translation_id: translation.into(),
            book_id: book.into(),
            chapter_number: chapter as i32,
            content: parse_verses(data.chapter.content),
        })
    }

    pub fn chapter_simple(
        &self,
        translation: &str,
        book: &str,
        chapter: u32,
    ) -> Result<FreeBibleChapter, String> {
        let data: SimpleChapterResponse =
            self.get_json(&format!("{}/{}/{}.simple.json", translation, book, chapter))?;
        Ok(FreeBibleChapter {
            translation_id: translation.into(),
            book_id: book.into(),
            chapter_number: chapter as i32,
            content: data
                .chapter
                .content
                .into_iter()
                .filter_map(|i| {
                    if let SimpleItem::Verse { number, text } = i {
                        Some(FreeBibleVerse { number, text })
                    } else {
                        None
                    }
                })
                .collect(),
        })
    }

    pub fn complete(
        &self,
        translation: &str,
        mut on_bytes: impl FnMut(u64, Option<u64>),
    ) -> Result<FreeBibleComplete, String> {
        use std::io::Read;

        let url = format!("{}/{}/complete.json", self.base_url, translation);
        let mut resp = self.http.get(&url).send().map_err(|e| e.to_string())?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {} for {}", resp.status(), url));
        }

        let total = resp.content_length();
        let mut buf = Vec::with_capacity(total.unwrap_or(8 * 1024 * 1024) as usize);
        let mut chunk = [0u8; 64 * 1024];
        let mut received: u64 = 0;

        loop {
            let n = resp.read(&mut chunk).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            received += n as u64;
            on_bytes(received, total);
        }

        let data: CompleteResponse = serde_json::from_slice(&buf).map_err(|e| e.to_string())?;

        let books = data
            .books
            .into_iter()
            .map(|b| {
                let raw_book_json = serde_json::to_string(&b.raw).unwrap_or_default();
                FreeBibleCompleteBook {
                    id: b.id,
                    name: b.name,
                    number_of_chapters: b.number_of_chapters,
                    common_name: b.common_name,
                    title: b.title,
                    book_order: b.order,
                    audio_links_json: b
                        .audio_links
                        .as_ref()
                        .and_then(|v| serde_json::to_string(v).ok()),
                    raw_json: raw_book_json,
                    chapters: b
                        .chapters
                        .into_iter()
                        .map(|wrap| {
                            let raw_chapter_json =
                                serde_json::to_string(&wrap.raw).unwrap_or_default();
                            let c = wrap.chapter;

                            let items = parse_chapter_items(c.content.clone());
                            let verses = items
                                .iter()
                                .filter_map(|i| match i.verse_number {
                                    Some(n) => Some(FreeBibleVerse {
                                        number: n,
                                        text: i.text.clone().unwrap_or_default(),
                                    }),
                                    None => None,
                                })
                                .collect();

                            let footnotes = c
                                .footnotes
                                .into_iter()
                                .map(|f| {
                                    let raw = serde_json::to_string(&f.raw).unwrap_or_default();
                                    FreeBibleFootnote {
                                        note_id: f.note_id,
                                        caller: f.caller,
                                        text: f.text,
                                        reference: f.reference,
                                        raw_json: raw,
                                    }
                                })
                                .collect();

                            let audio_timings = c
                                .audio_timings
                                .into_iter()
                                .map(|t| FreeBibleAudioTiming {
                                    verse: t.verse.unwrap_or(0),
                                    start_ms: t.start_ms.map(|x| x as i64),
                                    end_ms: t.end_ms.map(|x| x as i64),
                                })
                                .collect();

                            let raw_content_json =
                                serde_json::to_string(&c.content).unwrap_or_default();

                            FreeBibleCompleteChapter {
                                number: c.number,
                                verses,
                                items,
                                footnotes,
                                audio_timings,
                                raw_json: format!(
                                    r#"{{"chapter":{},"content":{}}}"#,
                                    raw_chapter_json, raw_content_json
                                ),
                            }
                        })
                        .collect(),
                }
            })
            .collect();

        Ok(FreeBibleComplete {
            translation_id: translation.to_string(),
            books,
        })
    }

    // ── Commentaries ──────────────────────────────────────────────

    pub fn commentaries(&self) -> Result<Vec<FreeBibleCommentary>, String> {
        let data: CommentariesResponse = self.get_json("available_commentaries.json")?;
        Ok(data
            .commentaries
            .into_iter()
            .map(|c| FreeBibleCommentary {
                id: c.id,
                name: c.name,
                english_name: c.english_name,
                language: c.language,
                license_url: c.license_url,
                license_notes: c.license_notes,
                number_of_books: c.number_of_books,
                total_number_of_chapters: c.total_number_of_chapters,
                total_number_of_verses: c.total_number_of_verses,
                raw_json: serde_json::to_string(&c.raw).unwrap_or_default(),
            })
            .collect())
    }

    pub fn commentary_books(&self, commentary: &str) -> Result<Vec<FreeBibleBook>, String> {
        let data: BooksResponse = self.get_json(&format!("c/{}/books.json", commentary))?;
        Ok(data
            .books
            .into_iter()
            .map(|b| FreeBibleBook {
                id: b.id,
                name: b.name,
                number_of_chapters: b.number_of_chapters,
                common_name: b.common_name,
                title: b.title,
                book_order: b.order,
                audio_links_json: None,
                raw_json: serde_json::to_string(&b.raw).ok(),
            })
            .collect())
    }

    pub fn commentary_chapter(
        &self,
        commentary: &str,
        book: &str,
        chapter: u32,
    ) -> Result<FreeBibleCommentaryChapter, String> {
        let data: ChapterResponse =
            self.get_json(&format!("c/{}/{}/{}.json", commentary, book, chapter))?;
        Ok(FreeBibleCommentaryChapter {
            commentary_id: commentary.into(),
            book_id: book.into(),
            chapter_number: chapter as i32,
            content: parse_verses(data.chapter.content),
        })
    }

    pub fn commentary_profiles(&self, commentary: &str) -> Result<Vec<CommentaryProfile>, String> {
        let data: ProfilesResponse = self.get_json(&format!("c/{}/profiles.json", commentary))?;
        Ok(data.profiles)
    }

    pub fn commentary_profile(
        &self,
        commentary: &str,
        profile: &str,
    ) -> Result<CommentaryProfile, String> {
        self.get_json(&format!("c/{}/profiles/{}.json", commentary, profile))
    }

    // ── Datasets ──────────────────────────────────────────────────

    pub fn datasets(&self) -> Result<Vec<FreeBibleDataset>, String> {
        let data: DatasetsResponse = self.get_json("available_datasets.json")?;
        Ok(data
            .datasets
            .into_iter()
            .map(|d| FreeBibleDataset {
                id: d.id,
                name: d.name.clone(),
                english_name: d.english_name.clone().unwrap_or(d.name),
                language: d.language.clone().unwrap_or_else(|| "eng".into()),
                license_url: d.license_url,
                license_notes: d.license_notes,
                total_number_of_books: d
                    .total_number_of_books
                    .or(d.total_number_of_words)
                    .or(d.total_number_of_lexicon)
                    .unwrap_or(0),
                raw_json: serde_json::to_string(&d.raw).unwrap_or_default(),
            })
            .collect())
    }

    pub fn dataset_books(&self, dataset: &str) -> Result<Vec<FreeBibleBook>, String> {
        let data: BooksResponse = self.get_json(&format!("d/{}/books.json", dataset))?;
        Ok(data
            .books
            .into_iter()
            .map(|b| FreeBibleBook {
                id: b.id,
                name: b.name,
                number_of_chapters: b.number_of_chapters,
                common_name: b.common_name,
                title: b.title,
                book_order: b.order,
                audio_links_json: None,
                raw_json: serde_json::to_string(&b.raw).ok(),
            })
            .collect())
    }

    pub fn dataset_chapter_raw(
        &self,
        dataset: &str,
        book: &str,
        chapter: u32,
    ) -> Result<serde_json::Value, String> {
        self.get_json(&format!("d/{}/{}/{}.json", dataset, book, chapter))
    }

    pub fn dataset_people_list(&self, dataset: &str) -> Result<Vec<EntitySummary>, String> {
        let data: PeopleListResponse = self.get_json(&format!("d/{}/people.json", dataset))?;
        Ok(data.people.into_iter().map(Into::into).collect())
    }

    pub fn dataset_events_list(&self, dataset: &str) -> Result<Vec<EntitySummary>, String> {
        let data: EventsListResponse = self.get_json(&format!("d/{}/events.json", dataset))?;
        Ok(data.events.into_iter().map(Into::into).collect())
    }

    pub fn dataset_places_list(&self, dataset: &str) -> Result<Vec<EntitySummary>, String> {
        let data: PlacesListResponse = self.get_json(&format!("d/{}/places.json", dataset))?;
        Ok(data.places.into_iter().map(Into::into).collect())
    }

    pub fn dataset_groups_list(&self, dataset: &str) -> Result<Vec<EntitySummary>, String> {
        let data: GroupsListResponse = self.get_json(&format!("d/{}/groups.json", dataset))?;
        Ok(data.groups.into_iter().map(Into::into).collect())
    }

    pub fn dataset_person(&self, dataset: &str, id: &str) -> Result<serde_json::Value, String> {
        self.get_json(&format!("d/{}/people/{}.json", dataset, id))
    }

    pub fn dataset_event(&self, dataset: &str, id: &str) -> Result<serde_json::Value, String> {
        self.get_json(&format!("d/{}/events/{}.json", dataset, id))
    }

    pub fn dataset_place(&self, dataset: &str, id: &str) -> Result<serde_json::Value, String> {
        self.get_json(&format!("d/{}/places/{}.json", dataset, id))
    }

    pub fn dataset_group(&self, dataset: &str, id: &str) -> Result<serde_json::Value, String> {
        self.get_json(&format!("d/{}/groups/{}.json", dataset, id))
    }

    pub fn lexicon_entry(
        &self,
        dataset: &str,
        strongs: &str,
    ) -> Result<Option<FreeBibleLexiconEntry>, String> {
        let url = format!("{}/d/{}/lexicon/{}.json", self.base_url, dataset, strongs);
        let resp = self.http.get(&url).send().map_err(|e| e.to_string())?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let data: LexiconEntryResponse = resp.json().map_err(|e| e.to_string())?;
        let sense = data.entry.senses.into_iter().next();
        Ok(Some(FreeBibleLexiconEntry {
            strongs: data.entry.id,
            lemma: sense.as_ref().and_then(|s| s.lemma.clone()),
            transliteration: sense.as_ref().and_then(|s| s.transliteration.clone()),
            part_of_speech: sense.as_ref().and_then(|s| s.part_of_speech.clone()),
            gloss: sense.as_ref().and_then(|s| s.gloss.clone()),
            definition: sense.as_ref().and_then(|s| s.definition.clone()),
        }))
    }

    // ── Helper ────────────────────────────────────────────────────

    fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        let url = format!("{}/{}", self.base_url, path);
        let resp = self.http.get(&url).send().map_err(|e| e.to_string())?;
        resp.json().map_err(|e| e.to_string())
    }
}

// ── Flexible deserializers ────────────────────────────────────────

fn de_bool_flexible<'de, D>(d: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let v: Option<serde_json::Value> = Option::deserialize(d)?;
    Ok(match v {
        Some(serde_json::Value::Bool(b)) => b,
        Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(0) != 0,
        _ => false,
    })
}

fn de_string_from_any<'de, D>(d: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let v = serde_json::Value::deserialize(d)?;
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
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let v = serde_json::Value::deserialize(d)?;
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
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Number(n) => n.as_i64().map(|x| x as i32),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    })
}

// ── Text assembly helper ──────────────────────────────────────────

fn append_piece(text: &mut String, piece: &str) {
    if piece.is_empty() {
        return;
    }
    if !text.is_empty() {
        let need_space = !text.ends_with([' ', '\n', '\t', '\u{00A0}'])
            && !piece.starts_with([' ', '\n', '\t', '\u{00A0}']);
        if need_space {
            text.push(' ');
        }
    }
    text.push_str(piece);
}

// ── Parsers ───────────────────────────────────────────────────────

pub(crate) fn parse_chapter_items(items: Vec<RawChapterItem>) -> Vec<FreeBibleChapterItem> {
    let mut out = Vec::with_capacity(items.len());
    for (pos, item) in items.into_iter().enumerate() {
        let position = pos as i32;
        match item {
            RawChapterItem::Verse { number, content } => {
                let mut text = String::new();
                let mut poem = false;
                let mut words_of_jesus = false;
                let mut footnote_ids: Vec<String> = Vec::new();
                for piece in &content {
                    match piece {
                        RawVersePiece::Text(t) => append_piece(&mut text, t),
                        RawVersePiece::Formatted(f) => {
                            append_piece(&mut text, &f.text);
                            poem |= f.poem;
                            words_of_jesus |= f.words_of_jesus;
                        }
                        RawVersePiece::Footnote(note) => {
                            if let Some(id) = &note.note_id {
                                footnote_ids.push(id.clone());
                            }
                        }
                        RawVersePiece::Other(_) => {}
                    }
                }
                let raw_json = serde_json::to_string(&RawChapterItem::Verse {
                    number,
                    content: content.clone(),
                })
                .unwrap_or_default();
                out.push(FreeBibleChapterItem {
                    position,
                    item_type: "verse".into(),
                    verse_number: Some(number),
                    text: Some(text),
                    poem,
                    words_of_jesus,
                    footnote_ids,
                    raw_json,
                });
            }
            RawChapterItem::Heading { content } => {
                let mut text = String::new();
                for piece in &content {
                    match piece {
                        RawVersePiece::Text(t) => append_piece(&mut text, t),
                        RawVersePiece::Formatted(f) => append_piece(&mut text, &f.text),
                        _ => {}
                    }
                }
                let raw_json = serde_json::to_string(&RawChapterItem::Heading {
                    content: content.clone(),
                })
                .unwrap_or_default();
                out.push(FreeBibleChapterItem {
                    position,
                    item_type: "heading".into(),
                    verse_number: None,
                    text: Some(text),
                    poem: false,
                    words_of_jesus: false,
                    footnote_ids: Vec::new(),
                    raw_json,
                });
            }
            RawChapterItem::HebrewSubtitle { content } => {
                let mut text = String::new();
                for piece in &content {
                    match piece {
                        RawVersePiece::Text(t) => append_piece(&mut text, t),
                        RawVersePiece::Formatted(f) => append_piece(&mut text, &f.text),
                        _ => {}
                    }
                }
                let raw_json = serde_json::to_string(&RawChapterItem::HebrewSubtitle {
                    content: content.clone(),
                })
                .unwrap_or_default();
                out.push(FreeBibleChapterItem {
                    position,
                    item_type: "hebrew_subtitle".into(),
                    verse_number: None,
                    text: Some(text),
                    poem: false,
                    words_of_jesus: false,
                    footnote_ids: Vec::new(),
                    raw_json,
                });
            }
            RawChapterItem::LineBreak => {
                out.push(FreeBibleChapterItem {
                    position,
                    item_type: "line_break".into(),
                    verse_number: None,
                    text: None,
                    poem: false,
                    words_of_jesus: false,
                    footnote_ids: Vec::new(),
                    raw_json: r#"{"type":"line_break"}"#.into(),
                });
            }
            RawChapterItem::Unknown => {
                out.push(FreeBibleChapterItem {
                    position,
                    item_type: "unknown".into(),
                    verse_number: None,
                    text: None,
                    poem: false,
                    words_of_jesus: false,
                    footnote_ids: Vec::new(),
                    raw_json: r#"{"type":"unknown"}"#.into(),
                });
            }
        }
    }
    out
}

pub(crate) fn parse_verses(items: Vec<RawChapterItem>) -> Vec<FreeBibleVerse> {
    parse_chapter_items(items)
        .into_iter()
        .filter_map(|item| match item.verse_number {
            Some(number) => Some(FreeBibleVerse {
                number,
                text: item.text.unwrap_or_default(),
            }),
            None => None,
        })
        .collect()
}

// ── Serde structs ─────────────────────────────────────────────────

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
    text_direction: String,
    #[serde(rename = "numberOfBooks")]
    number_of_books: i32,
    #[serde(rename = "totalNumberOfChapters")]
    total_number_of_chapters: i32,
    #[serde(rename = "totalNumberOfVerses")]
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

#[derive(Deserialize)]
struct RawBook {
    id: String,
    name: String,
    #[serde(rename = "numberOfChapters")]
    number_of_chapters: i32,
    #[serde(rename = "commonName", default, deserialize_with = "de_opt_string")]
    common_name: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    title: Option<String>,
    #[serde(default, deserialize_with = "de_opt_i32")]
    order: Option<i32>,
    #[serde(rename = "thisBookAudioLinks")]
    audio_links: Option<serde_json::Value>,
    #[serde(flatten)]
    raw: serde_json::Value,
}

#[derive(Deserialize)]
struct CommentariesResponse {
    commentaries: Vec<RawCommentary>,
}

#[derive(Deserialize)]
struct RawCommentary {
    id: String,
    name: String,
    #[serde(rename = "englishName")]
    english_name: String,
    language: String,
    #[serde(rename = "licenseUrl")]
    license_url: Option<String>,
    #[serde(rename = "licenseNotes")]
    license_notes: Option<String>,
    #[serde(rename = "numberOfBooks")]
    number_of_books: i32,
    #[serde(rename = "totalNumberOfChapters")]
    total_number_of_chapters: i32,
    #[serde(rename = "totalNumberOfVerses")]
    total_number_of_verses: i32,
    #[serde(flatten)]
    raw: serde_json::Value,
}

#[derive(Deserialize)]
struct ProfilesResponse {
    profiles: Vec<CommentaryProfile>,
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
    #[serde(flatten)]
    raw: serde_json::Value,
}

#[derive(Deserialize)]
struct LexiconEntryResponse {
    entry: RawLexiconEntry,
}

#[derive(Deserialize)]
struct RawLexiconEntry {
    id: String,
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

#[derive(Deserialize)]
struct ChapterResponse {
    chapter: RawChapter,
}

#[derive(Deserialize)]
struct RawChapter {
    content: Vec<RawChapterItem>,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(tag = "type")]
pub(crate) enum RawChapterItem {
    #[serde(rename = "verse")]
    Verse {
        number: i32,
        content: Vec<RawVersePiece>,
    },
    #[serde(rename = "heading")]
    Heading { content: Vec<RawVersePiece> },
    #[serde(rename = "line_break")]
    LineBreak,
    #[serde(rename = "hebrew_subtitle")]
    HebrewSubtitle { content: Vec<RawVersePiece> },
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(untagged)]
pub(crate) enum RawVersePiece {
    Text(String),
    Formatted(FormattedText),
    Footnote(FootnoteRef),
    Other(serde_json::Value),
}

#[derive(Deserialize, Serialize, Clone)]
pub(crate) struct FormattedText {
    pub text: String,
    #[serde(default, deserialize_with = "de_bool_flexible")]
    pub poem: bool,
    #[serde(
        rename = "wordsOfJesus",
        default,
        deserialize_with = "de_bool_flexible"
    )]
    pub words_of_jesus: bool,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct FootnoteRef {
    #[serde(rename = "noteId", default, deserialize_with = "de_opt_string")]
    pub note_id: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    pub caller: Option<String>,
}

#[derive(Deserialize)]
struct SimpleChapterResponse {
    chapter: SimpleChapter,
}

#[derive(Deserialize)]
struct SimpleChapter {
    content: Vec<SimpleItem>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum SimpleItem {
    #[serde(rename = "verse")]
    Verse { number: i32, text: String },
    #[serde(other)]
    Other,
}

// ── Complete translation serde ────────────────────────────────────

#[derive(Deserialize)]
struct CompleteResponse {
    books: Vec<RawCompleteBook>,
}

#[derive(Deserialize)]
struct RawCompleteBook {
    id: String,
    name: String,
    #[serde(rename = "numberOfChapters")]
    number_of_chapters: i32,
    #[serde(rename = "commonName", default, deserialize_with = "de_opt_string")]
    common_name: Option<String>,
    #[serde(default, deserialize_with = "de_opt_string")]
    title: Option<String>,
    #[serde(default, deserialize_with = "de_opt_i32")]
    order: Option<i32>,
    #[serde(rename = "thisBookAudioLinks")]
    audio_links: Option<serde_json::Value>,
    chapters: Vec<RawCompleteChapterWrap>,
    #[serde(flatten)]
    raw: serde_json::Value,
}

#[derive(Deserialize)]
struct RawCompleteChapterWrap {
    chapter: RawCompleteChapter,
    #[serde(flatten)]
    raw: serde_json::Value,
}

#[derive(Deserialize)]
struct RawCompleteChapter {
    number: i32,
    content: Vec<RawChapterItem>,
    #[serde(default)]
    footnotes: Vec<RawFootnote>,
    #[serde(rename = "thisChapterAudioTimings", default)]
    audio_timings: Vec<RawAudioTiming>,
}

#[derive(Deserialize)]
struct RawFootnote {
    #[serde(rename = "noteId", deserialize_with = "de_string_from_any")]
    note_id: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    caller: Option<String>,
    #[serde(default, deserialize_with = "de_string_from_any")]
    text: String,
    #[serde(default, deserialize_with = "de_opt_string")]
    reference: Option<String>,
    #[serde(flatten)]
    raw: serde_json::Value,
}

#[derive(Deserialize)]
struct RawAudioTiming {
    #[serde(default, deserialize_with = "de_opt_i32")]
    verse: Option<i32>,
    #[serde(rename = "start", default)]
    start_ms: Option<f64>,
    #[serde(rename = "end", default)]
    end_ms: Option<f64>,
}
