use rusqlite::{Connection, OptionalExtension};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

use crate::error::XBibleError;
use crate::sources::models::{
    CanonicalSection, GenericAudioSource, GenericAudioTiming, GenericBook, GenericChapterItem,
    GenericFootnote, GenericTextSpan, GenericTranslation, LexicalInfo, SpanFormatting, SpanKind,
    TextDirection,
};

pub mod search;

// ── Supporting Reader Structs ──────────────────────────────────

#[derive(Debug, Clone, uniffi::Record)]
pub struct BookSection {
    pub section: CanonicalSection,
    pub section_key: String,
    pub books: Vec<GenericBook>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct EntitySummary {
    pub id: String,
    pub name: String,
    pub is_proper_name: Option<bool>,
    pub gender: Option<String>,
    pub start_date: Option<String>,
    pub number_of_references: i64,
}

// ── Scholarly Reader Chapter & Verse Models ────────────────────

#[derive(Debug, Clone, uniffi::Record)]
pub struct ReaderVerse {
    pub number: i32,
    pub text: String,
    pub spans: Vec<GenericTextSpan>,
    pub footnote_ids: Vec<String>,
    pub cross_ref_ids: Vec<String>,
    pub audio_timing: Option<GenericAudioTiming>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ReaderBlock {
    pub position: i32,
    pub item_type: String,
    pub verse_number: Option<i32>,
    pub text: String,
    pub is_poem: bool,
    pub is_words_of_jesus: bool,
    pub spans: Vec<GenericTextSpan>,
    pub verses: Vec<ReaderVerse>,
    pub footnote_ids: Vec<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ReaderChapter {
    pub translation_id: String,
    pub book_id: String,
    pub book_name: String,
    pub chapter_number: i32,
    pub text_direction: TextDirection,
    pub items: Vec<GenericChapterItem>,
    pub blocks: Vec<ReaderBlock>,
    pub footnotes: Vec<GenericFootnote>,
    pub audio_timings: Vec<GenericAudioTiming>,
    pub next_chapter: Option<i32>,
    pub prev_chapter: Option<i32>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct CommentaryBlock {
    pub verse_number: i32,
    pub text: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct CommentaryChapter {
    pub commentary_id: String,
    pub book_id: String,
    pub book_name: String,
    pub chapter_number: i32,
    pub blocks: Vec<CommentaryBlock>,
    pub next_chapter: Option<i32>,
    pub prev_chapter: Option<i32>,
}

// ── Entity Models ──────────────────────────────────────────────

#[derive(Debug, Clone, uniffi::Record)]
pub struct EntityRef {
    pub kind: String,
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct VerseRange {
    pub book: String,
    pub chapter: i32,
    pub verse: i32,
    pub end_verse: Option<i32>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct PersonDetail {
    pub id: String,
    pub name: String,
    pub also_called: Vec<String>,
    pub gender: Option<String>,
    pub birth_year: Option<i64>,
    pub death_year: Option<i64>,
    pub min_year: Option<i64>,
    pub max_year: Option<i64>,
    pub description: Option<String>,
    pub father: Vec<EntityRef>,
    pub mother: Vec<EntityRef>,
    pub partners: Vec<EntityRef>,
    pub children: Vec<EntityRef>,
    pub siblings: Vec<EntityRef>,
    pub half_siblings_same_father: Vec<EntityRef>,
    pub half_siblings_same_mother: Vec<EntityRef>,
    pub member_of: Vec<EntityRef>,
    pub birth_place: Vec<EntityRef>,
    pub death_place: Vec<EntityRef>,
    pub events: Vec<EntityRef>,
    pub referenced_by_events: Vec<EntityRef>,
    pub verses: Vec<VerseRange>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct EventDetail {
    pub id: String,
    pub name: String,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub duration: Option<String>,
    pub description: Option<String>,
    pub participants: Vec<EntityRef>,
    pub locations: Vec<EntityRef>,
    pub predecessor: Option<EntityRef>,
    pub successor: Option<EntityRef>,
    pub parent_event: Option<EntityRef>,
    pub sub_events: Vec<EntityRef>,
    pub verses: Vec<VerseRange>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct PlaceDetail {
    pub id: String,
    pub name: String,
    pub kjv_name: Option<String>,
    pub esv_name: Option<String>,
    pub feature_type: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub description: Option<String>,
    pub events: Vec<EntityRef>,
    pub people_born_here: Vec<EntityRef>,
    pub people_died_here: Vec<EntityRef>,
    pub people_groups_here: Vec<EntityRef>,
    pub verses: Vec<VerseRange>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct GroupDetail {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub members: Vec<EntityRef>,
    pub leaders: Vec<EntityRef>,
    pub located_at: Vec<EntityRef>,
    pub verses: Vec<VerseRange>,
}

// ── DB & Span Normalization Helpers ────────────────────────────

fn parse_canonical_section(raw: &str) -> CanonicalSection {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ot" | "old_testament" | "oldtestament" => CanonicalSection::OldTestament,
        "nt" | "new_testament" | "newtestament" => CanonicalSection::NewTestament,
        "deutero" | "deuterocanon" | "apocrypha" => CanonicalSection::Deuterocanon,
        "orthodox_extra" | "orthodoxextra" => CanonicalSection::OrthodoxExtra,
        "ethiopian_extra" | "ethiopianextra" => CanonicalSection::EthiopianExtra,
        "unknown" | "" => CanonicalSection::Unknown,
        other => CanonicalSection::Custom(other.to_string()),
    }
}

fn section_wire_key(section: &CanonicalSection) -> String {
    match section {
        CanonicalSection::OldTestament => "ot".to_string(),
        CanonicalSection::NewTestament => "nt".to_string(),
        CanonicalSection::Deuterocanon => "deutero".to_string(),
        CanonicalSection::OrthodoxExtra => "orthodox_extra".to_string(),
        CanonicalSection::EthiopianExtra => "ethiopian_extra".to_string(),
        CanonicalSection::Unknown => "unknown".to_string(),
        CanonicalSection::Custom(label) => label.clone(),
    }
}

fn parse_text_direction(raw: &str) -> TextDirection {
    match raw.trim().to_ascii_lowercase().as_str() {
        "rtl" | "right_to_left" | "righttoleft" => TextDirection::RightToLeft,
        _ => TextDirection::LeftToRight,
    }
}

fn parse_span_kind(raw: Option<&str>, has_lexical: bool) -> SpanKind {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("word") => SpanKind::Word,
        Some("footnote_ref") | Some("footnoteref") => SpanKind::FootnoteRef,
        Some("cross_ref") | Some("crossref") => SpanKind::CrossRef,
        Some("quote") => SpanKind::Quote,
        Some("text") => SpanKind::Text,
        Some(_) => SpanKind::Other,
        None if has_lexical => SpanKind::Word,
        None => SpanKind::Text,
    }
}

fn parse_string_array(val: Option<&serde_json::Value>) -> Vec<String> {
    val.and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_lexical_info(obj: &serde_json::Map<String, serde_json::Value>) -> Option<LexicalInfo> {
    let lex_obj = obj
        .get("lexical")
        .and_then(|v| v.as_object())
        .unwrap_or(obj);

    let strongs = lex_obj
        .get("strongs")
        .or_else(|| lex_obj.get("strong"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let lemma = lex_obj
        .get("lemma")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let transliteration = lex_obj
        .get("transliteration")
        .or_else(|| lex_obj.get("translit"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let part_of_speech = lex_obj
        .get("part_of_speech")
        .or_else(|| lex_obj.get("partOfSpeech"))
        .or_else(|| lex_obj.get("pos"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let morphology = lex_obj
        .get("morphology")
        .or_else(|| lex_obj.get("morph"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let gloss = lex_obj
        .get("gloss")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    if strongs.is_none()
        && lemma.is_none()
        && transliteration.is_none()
        && part_of_speech.is_none()
        && morphology.is_none()
        && gloss.is_none()
    {
        None
    } else {
        Some(LexicalInfo {
            strongs,
            lemma,
            transliteration,
            part_of_speech,
            morphology,
            gloss,
        })
    }
}

fn parse_span_formatting(
    obj: &serde_json::Map<String, serde_json::Value>,
    default_woj: bool,
    default_poetry: bool,
) -> SpanFormatting {
    let fmt_obj = obj
        .get("formatting")
        .and_then(|v| v.as_object())
        .unwrap_or(obj);

    let bool_field = |keys: &[&str], default: bool| -> bool {
        for key in keys {
            if let Some(b) = fmt_obj.get(*key).and_then(|v| v.as_bool()) {
                return b;
            }
            if let Some(b) = obj.get(*key).and_then(|v| v.as_bool()) {
                return b;
            }
        }
        default
    };

    SpanFormatting {
        is_words_of_jesus: bool_field(
            &["is_words_of_jesus", "isWordsOfJesus", "wordsOfJesus"],
            default_woj,
        ),
        is_italic: bool_field(&["is_italic", "isItalic", "italic"], false),
        is_bold: bool_field(&["is_bold", "isBold", "bold"], false),
        is_small_caps: bool_field(&["is_small_caps", "isSmallCaps", "smallCaps"], false),
        is_underline: bool_field(&["is_underline", "isUnderline", "underline"], false),
        is_poetry: bool_field(&["is_poetry", "isPoetry", "poem"], default_poetry),
    }
}

fn parse_text_span(
    piece: &serde_json::Value,
    fallback_pos: i32,
    default_woj: bool,
    default_poetry: bool,
) -> Option<GenericTextSpan> {
    if let Some(s) = piece.as_str() {
        return Some(GenericTextSpan {
            position: fallback_pos,
            kind: SpanKind::Text,
            text: s.to_string(),
            formatting: SpanFormatting {
                is_words_of_jesus: default_woj,
                is_poetry: default_poetry,
                ..SpanFormatting::default()
            },
            lexical: None,
            footnote_ids: Vec::new(),
            cross_ref_ids: Vec::new(),
            language_code: None,
            raw_json: None,
        });
    }

    let obj = piece.as_object()?;
    let position = obj
        .get("position")
        .and_then(|v| v.as_i64())
        .map(|n| n as i32)
        .unwrap_or(fallback_pos);

    let text = obj
        .get("text")
        .or_else(|| obj.get("word"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let lexical = parse_lexical_info(obj);
    let kind = parse_span_kind(obj.get("kind").and_then(|v| v.as_str()), lexical.is_some());
    let formatting = parse_span_formatting(obj, default_woj, default_poetry);

    let mut footnote_ids =
        parse_string_array(obj.get("footnote_ids").or_else(|| obj.get("footnoteIds")));
    if let Some(single) = obj
        .get("noteId")
        .or_else(|| obj.get("footnoteId"))
        .and_then(|v| v.as_str())
    {
        if !footnote_ids.iter().any(|x| x == single) {
            footnote_ids.push(single.to_string());
        }
    }

    let cross_ref_ids =
        parse_string_array(obj.get("cross_ref_ids").or_else(|| obj.get("crossRefIds")));

    let language_code = obj
        .get("language_code")
        .or_else(|| obj.get("languageCode"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let raw_json = obj
        .get("raw_json")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| Some(piece.to_string()));

    Some(GenericTextSpan {
        position,
        kind,
        text,
        formatting,
        lexical,
        footnote_ids,
        cross_ref_ids,
        language_code,
        raw_json,
    })
}

fn parse_spans_from_value(
    val: &serde_json::Value,
    default_woj: bool,
    default_poetry: bool,
) -> Vec<GenericTextSpan> {
    let Some(arr) = val
        .get("spans")
        .or_else(|| val.get("content"))
        .or_else(|| val.get("words"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };

    arr.iter()
        .enumerate()
        .filter_map(|(idx, piece)| parse_text_span(piece, idx as i32, default_woj, default_poetry))
        .collect()
}

fn fallback_span(text: &str, is_woj: bool, is_poetry: bool) -> Vec<GenericTextSpan> {
    if text.is_empty() {
        return Vec::new();
    }
    vec![GenericTextSpan {
        position: 0,
        kind: SpanKind::Text,
        text: text.to_string(),
        formatting: SpanFormatting {
            is_words_of_jesus: is_woj,
            is_poetry,
            ..SpanFormatting::default()
        },
        lexical: None,
        footnote_ids: Vec::new(),
        cross_ref_ids: Vec::new(),
        language_code: None,
        raw_json: None,
    }]
}

fn extract_chapter_item_spans(
    raw_json: Option<&str>,
    fallback_text: &str,
    is_woj: bool,
    is_poetry: bool,
) -> Vec<GenericTextSpan> {
    if let Some(raw) = raw_json {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) {
            let spans = parse_spans_from_value(&json, is_woj, is_poetry);
            if !spans.is_empty() {
                return spans;
            }
        }
    }
    fallback_span(fallback_text, is_woj, is_poetry)
}

fn build_reader_verses(
    raw_json: Option<&str>,
    default_number: i32,
    fallback_text: &str,
    item_spans: &[GenericTextSpan],
    item_footnotes: &[String],
    is_woj: bool,
    is_poetry: bool,
    timings_by_verse: &BTreeMap<i32, GenericAudioTiming>,
) -> Vec<ReaderVerse> {
    if let Some(raw) = raw_json {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) {
            if let Some(verses_arr) = json.get("verses").and_then(|v| v.as_array()) {
                let mut out = Vec::new();
                for v_val in verses_arr {
                    let num = v_val
                        .get("number")
                        .or_else(|| v_val.get("verse"))
                        .and_then(|v| v.as_i64())
                        .map(|n| n as i32)
                        .unwrap_or(default_number);

                    let mut spans = parse_spans_from_value(v_val, is_woj, is_poetry);
                    let v_text = v_val
                        .get("text")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            spans.iter().map(|s| s.text.as_str()).collect::<String>()
                        });
                    if spans.is_empty() {
                        spans = fallback_span(&v_text, is_woj, is_poetry);
                    }

                    let mut f_ids = parse_string_array(
                        v_val
                            .get("footnote_ids")
                            .or_else(|| v_val.get("footnoteIds")),
                    );
                    if f_ids.is_empty() {
                        f_ids = item_footnotes.to_vec();
                    }
                    for s in &spans {
                        for fid in &s.footnote_ids {
                            if !f_ids.contains(fid) {
                                f_ids.push(fid.clone());
                            }
                        }
                    }

                    let mut xref_ids = parse_string_array(
                        v_val
                            .get("cross_ref_ids")
                            .or_else(|| v_val.get("crossRefIds")),
                    );
                    for s in &spans {
                        for xid in &s.cross_ref_ids {
                            if !xref_ids.contains(xid) {
                                xref_ids.push(xid.clone());
                            }
                        }
                    }

                    out.push(ReaderVerse {
                        number: num,
                        text: v_text,
                        spans,
                        footnote_ids: f_ids,
                        cross_ref_ids: xref_ids,
                        audio_timing: timings_by_verse.get(&num).cloned(),
                    });
                }
                if !out.is_empty() {
                    return out;
                }
            }
        }
    }

    let spans = if item_spans.is_empty() {
        fallback_span(fallback_text, is_woj, is_poetry)
    } else {
        item_spans.to_vec()
    };

    let resolved_text = if fallback_text.is_empty() {
        spans.iter().map(|s| s.text.as_str()).collect::<String>()
    } else {
        fallback_text.to_string()
    };

    let mut f_ids = item_footnotes.to_vec();
    let mut xref_ids = Vec::new();
    for s in &spans {
        for fid in &s.footnote_ids {
            if !f_ids.contains(fid) {
                f_ids.push(fid.clone());
            }
        }
        for xid in &s.cross_ref_ids {
            if !xref_ids.contains(xid) {
                xref_ids.push(xid.clone());
            }
        }
    }

    vec![ReaderVerse {
        number: default_number,
        text: resolved_text,
        spans,
        footnote_ids: f_ids,
        cross_ref_ids: xref_ids,
        audio_timing: timings_by_verse.get(&default_number).cloned(),
    }]
}

fn read_forward_refs(
    conn: &Connection,
    dataset_id: &str,
    kind: &str,
    id: &str,
    relations: &[&str],
) -> Vec<EntityRef> {
    if relations.is_empty() {
        return Vec::new();
    }
    let placeholders = relations
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 4))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT target_kind, target_id, COALESCE(target_name, target_id)
         FROM entity_refs WHERE dataset_id = ?1 AND source_kind = ?2 AND source_id = ?3
           AND relation IN ({placeholders}) ORDER BY relation, target_name"
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut params: Vec<&dyn rusqlite::ToSql> = vec![&dataset_id, &kind, &id];
    for r in relations {
        params.push(r);
    }
    match stmt.query_map(&params[..], |row| {
        Ok(EntityRef {
            kind: row.get(0)?,
            id: row.get(1)?,
            name: row.get(2)?,
        })
    }) {
        Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

fn read_reverse_refs(
    conn: &Connection,
    dataset_id: &str,
    target_kind: &str,
    target_id: &str,
    relations: &[&str],
) -> Vec<EntityRef> {
    if relations.is_empty() {
        return Vec::new();
    }
    let placeholders = relations
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 4))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT source_kind, source_id,
                COALESCE((SELECT name FROM people WHERE id = source_id AND dataset_id = ?1),
                         (SELECT name FROM events WHERE id = source_id AND dataset_id = ?1),
                         (SELECT name FROM places WHERE id = source_id AND dataset_id = ?1),
                         (SELECT name FROM people_groups WHERE id = source_id AND dataset_id = ?1),
                         source_id)
         FROM entity_refs WHERE dataset_id = ?1 AND target_kind = ?2 AND target_id = ?3
           AND relation IN ({placeholders}) ORDER BY relation"
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut params: Vec<&dyn rusqlite::ToSql> = vec![&dataset_id, &target_kind, &target_id];
    for r in relations {
        params.push(r);
    }
    match stmt.query_map(&params[..], |row| {
        Ok(EntityRef {
            kind: row.get(0)?,
            id: row.get(1)?,
            name: row.get(2)?,
        })
    }) {
        Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

fn read_verse_refs(conn: &Connection, dataset_id: &str, kind: &str, id: &str) -> Vec<VerseRange> {
    let mut stmt = match conn.prepare(
        "SELECT book, chapter, verse, end_verse FROM entity_verse_refs
         WHERE dataset_id = ?1 AND source_kind = ?2 AND source_id = ?3 ORDER BY book, chapter, verse",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    match stmt.query_map(rusqlite::params![dataset_id, kind, id], |row| {
        Ok(VerseRange {
            book: row.get(0)?,
            chapter: row.get(1)?,
            verse: row.get(2)?,
            end_verse: row.get(3)?,
        })
    }) {
        Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

fn load_chapter_audio_timings(
    conn: &Connection,
    translation_id: &str,
    book_id: &str,
    chapter: i32,
) -> Vec<GenericAudioTiming> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT verse, start_ms, end_ms
         FROM chapter_audio_timings
         WHERE translation_id = ?1 AND book_id = ?2 AND chapter = ?3
         ORDER BY verse",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map(rusqlite::params![translation_id, book_id, chapter], |r| {
        Ok(GenericAudioTiming {
            verse: r.get(0)?,
            start_ms: r.get(1)?,
            end_ms: r.get(2)?,
        })
    }) else {
        return Vec::new();
    };
    rows.filter_map(|r| r.ok()).collect()
}

#[uniffi::export(callback_interface)]
pub trait SearchProgressListener: Send + Sync {
    fn on_progress(&self, progress: search::SearchProgress);
}

// ── Reader UniFFI Object ─────────────────────────────────────────

#[derive(Debug, uniffi::Object)]
pub struct XBibleReader {
    conn: RwLock<Option<Arc<Mutex<Connection>>>>,
}

#[uniffi::export]
impl XBibleReader {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            conn: RwLock::new(None),
        })
    }

    pub fn set_db_path(&self, path: String) -> Result<(), XBibleError> {
        let conn = Connection::open(&path).map_err(|e| XBibleError::Generic {
            error_message: e.to_string(),
        })?;
        let mut lock = self.conn.write().unwrap_or_else(|p| p.into_inner());
        *lock = Some(Arc::new(Mutex::new(conn)));
        Ok(())
    }

    // ── Search ─────────────────────────────────────

    pub fn search(
        &self,
        query: String,
        translation_id: Option<String>,
        limit: i32,
        filter: search::SearchFilter,
        listener: Option<Box<dyn SearchProgressListener>>,
    ) -> Result<search::SearchResults, XBibleError> {
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => {
                return Err(XBibleError::Generic {
                    error_message: "reader has no connection — call setDbPath first".into(),
                });
            }
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let emit = |p: search::SearchProgress| {
            if let Some(l) = listener.as_ref() {
                l.on_progress(p);
            }
        };

        search::search(
            &conn,
            &query,
            translation_id.as_deref(),
            limit,
            filter,
            emit,
        )
    }

    // ── Translations & Books ─────────────────────────────────────

    pub fn get_installed_translations(&self) -> Vec<GenericTranslation> {
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => {
                log::warn!("get_installed_translations: reader has no connection");
                return Vec::new();
            }
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let mut stmt = match conn.prepare(
            "SELECT id, COALESCE(source_id, ''), name, english_name, short_name,
                    COALESCE(language_code, language), language_name, language_english_name,
                    text_direction,
                    COALESCE(book_count, number_of_books),
                    COALESCE(chapter_count, total_number_of_chapters),
                    COALESCE(verse_count, total_number_of_verses),
                    installed, COALESCE(has_strongs, 0),
                    license_url, license_notes, raw_json
             FROM translations WHERE installed = 1 ORDER BY name",
        ) {
            Ok(s) => s,
            Err(e) => {
                log::error!("get_installed_translations prepare failed: {}", e);
                return Vec::new();
            }
        };

        let rows = stmt.query_map([], |r| {
            let dir_raw: String = r.get(8)?;
            Ok(GenericTranslation {
                id: r.get(0)?,
                source_id: r.get(1)?,
                name: r.get(2)?,
                english_name: r.get(3)?,
                short_name: r.get(4)?,
                language_code: r.get(5)?,
                language_name: r.get(6)?,
                language_english_name: r.get(7)?,
                text_direction: parse_text_direction(&dir_raw),
                book_count: r.get(9)?,
                chapter_count: r.get(10)?,
                verse_count: r.get(11)?,
                installed: r.get::<_, i32>(12)? != 0,
                has_strongs: r.get::<_, i32>(13)? != 0,
                license_url: r.get(14)?,
                license_notes: r.get(15)?,
                raw_json: r.get(16)?,
            })
        });

        match rows {
            Ok(r) => r
                .filter_map(|x| match x {
                    Ok(v) => Some(v),
                    Err(e) => {
                        log::error!("get_installed_translations row failed: {}", e);
                        None
                    }
                })
                .collect(),
            Err(e) => {
                log::error!("get_installed_translations query failed: {}", e);
                Vec::new()
            }
        }
    }

    fn parse_audio_sources_from_json(
        &self,
        audio_links_json: Option<String>,
    ) -> Vec<GenericAudioSource> {
        let Some(raw) = audio_links_json else {
            return Vec::new();
        };
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return Vec::new();
        };
        match val {
            serde_json::Value::Object(map) => map
                .into_iter()
                .filter_map(|(k, v)| {
                    v.as_str().map(|url| GenericAudioSource {
                        source_ref_id: k,
                        url: Some(url.to_string()),
                        format: None,
                        is_dramatized: None,
                    })
                })
                .collect(),
            serde_json::Value::Array(arr) => arr
                .into_iter()
                .enumerate()
                .filter_map(|(idx, v)| {
                    v.as_str().map(|url| GenericAudioSource {
                        source_ref_id: format!("track_{idx}"),
                        url: Some(url.to_string()),
                        format: None,
                        is_dramatized: None,
                    })
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn get_books(&self, translation_id: String) -> Vec<BookSection> {
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let source_id: String = conn
            .query_row(
                "SELECT COALESCE(NULLIF(source_id, ''), 'helloao') FROM translations WHERE id = ?1",
                rusqlite::params![translation_id],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| "helloao".to_string());

        let sql = "SELECT b.id, b.name, b.number_of_chapters,
                          m.common_name, m.title, m.book_order,
                          m.audio_links_json, m.raw_json,
                          COALESCE(m.section, 'unknown')
                   FROM books b
                   LEFT JOIN book_metadata m
                     ON m.translation_id = b.translation_id AND m.book_id = b.id
                   WHERE b.translation_id = ?1
                   ORDER BY COALESCE(m.book_order, 999999), b.id";

        let mut stmt = match conn.prepare(sql) {
            Ok(s) => s,
            Err(e) => {
                log::error!("get_books prepare failed: {e}");
                return Vec::new();
            }
        };

        let tid_ref = translation_id.clone();
        let sid_ref = source_id.clone();
        let rows = stmt.query_map(rusqlite::params![translation_id], |r| {
            let section_raw: String = r.get(8)?;
            let audio_sources = self.parse_audio_sources_from_json(r.get(6)?);

            Ok(GenericBook {
                translation_id: tid_ref.clone(),
                source_id: sid_ref.clone(),
                id: r.get(0)?,
                name: r.get(1)?,
                number_of_chapters: r.get(2)?,
                common_name: r.get(3)?,
                title: r.get(4)?,
                book_order: r.get(5)?,
                audio_sources,
                audio_links_json: r.get(6)?,
                raw_json: r.get(7)?,
                section: parse_canonical_section(&section_raw),
            })
        });

        let books: Vec<GenericBook> = match rows {
            Ok(r) => r.filter_map(|x| x.ok()).collect(),
            Err(_) => Vec::new(),
        };

        let canonical_order = [
            "ot",
            "deutero",
            "orthodox_extra",
            "ethiopian_extra",
            "unknown",
            "nt",
        ];

        let mut grouped: BTreeMap<String, Vec<GenericBook>> = BTreeMap::new();
        for book in books {
            let key = section_wire_key(&book.section);
            grouped.entry(key).or_default().push(book);
        }

        let mut result = Vec::new();
        for key in canonical_order {
            if let Some(books) = grouped.remove(key) {
                if !books.is_empty() {
                    result.push(BookSection {
                        section: parse_canonical_section(key),
                        section_key: key.to_string(),
                        books,
                    });
                }
            }
        }
        for (key, books) in grouped {
            if !books.is_empty() {
                result.push(BookSection {
                    section: parse_canonical_section(&key),
                    section_key: key,
                    books,
                });
            }
        }

        result
    }

    pub fn needs_section_update(&self, translation_id: String) -> Result<bool, XBibleError> {
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => {
                return Err(XBibleError::Generic {
                    error_message: "reader has no connection — call setDbPath first".into(),
                });
            }
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let unknown_count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM book_metadata
                 WHERE translation_id = ?1 AND section = 'unknown'",
                rusqlite::params![translation_id],
                |r| r.get(0),
            )
            .map_err(|e| XBibleError::Generic {
                error_message: format!("needs_section_update query failed: {e}"),
            })?;

        Ok(unknown_count > 0)
    }

    pub fn set_selected_canon(
        &self,
        translation_id: String,
        canon: String,
    ) -> Result<(), XBibleError> {
        const VALID: &[&str] = &["protestant", "catholic", "orthodox", "ethiopian", "all"];
        if !VALID.contains(&canon.as_str()) {
            return Err(XBibleError::Generic {
                error_message: format!("unknown canon {canon:?} — expected one of {VALID:?}"),
            });
        }

        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => {
                return Err(XBibleError::Generic {
                    error_message: "reader has no connection — call setDbPath first".into(),
                });
            }
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let n = conn
            .execute(
                "UPDATE translations SET selected_canon = ?1 WHERE id = ?2",
                rusqlite::params![canon, translation_id],
            )
            .map_err(|e| XBibleError::Generic {
                error_message: format!("set_selected_canon update failed: {e}"),
            })?;
        if n == 0 {
            return Err(XBibleError::Generic {
                error_message: format!("translation {translation_id} not found"),
            });
        }
        Ok(())
    }

    // ── Chapter Reading ──────────────────────────────────────────

    pub fn read_chapter(
        &self,
        translation_id: String,
        book_id: String,
        chapter: i32,
    ) -> Option<ReaderChapter> {
        let conn_arc = self
            .conn
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()?;
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let book_name: String = conn
            .query_row(
                "SELECT name FROM books WHERE translation_id = ?1 AND id = ?2",
                rusqlite::params![translation_id, book_id],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| book_id.clone());

        let text_direction: TextDirection = conn
            .query_row(
                "SELECT text_direction FROM translations WHERE id = ?1",
                rusqlite::params![translation_id],
                |r| r.get::<_, String>(0),
            )
            .map(|d| parse_text_direction(&d))
            .unwrap_or(TextDirection::LeftToRight);

        let mut f_stmt = conn
            .prepare(
                "SELECT note_id, caller, text, reference, raw_json
                 FROM footnotes WHERE translation_id = ?1 AND book_id = ?2 AND chapter = ?3",
            )
            .ok()?;

        let footnotes: Vec<GenericFootnote> = f_stmt
            .query_map(rusqlite::params![translation_id, book_id, chapter], |r| {
                Ok(GenericFootnote {
                    note_id: r.get(0)?,
                    caller: r.get(1)?,
                    text: r.get(2)?,
                    reference: r.get(3)?,
                    raw_json: r.get(4)?,
                })
            })
            .ok()?
            .filter_map(|r| r.ok())
            .collect();

        let audio_timings = load_chapter_audio_timings(&conn, &translation_id, &book_id, chapter);
        let timings_by_verse: BTreeMap<i32, GenericAudioTiming> =
            audio_timings.iter().map(|t| (t.verse, t.clone())).collect();

        let mut stmt = conn
            .prepare(
                "SELECT position, item_type, verse_number, text, poem, words_of_jesus, footnote_ids, raw_json
                 FROM chapter_items WHERE translation_id = ?1 AND book_id = ?2 AND chapter = ?3 ORDER BY position",
            )
            .ok()?;

        let rows = stmt
            .query_map(rusqlite::params![translation_id, book_id, chapter], |r| {
                let footnotes_str: Option<String> = r.get(6)?;
                let footnote_ids: Vec<String> = footnotes_str
                    .map(|s| {
                        s.split(',')
                            .filter(|x| !x.is_empty())
                            .map(|x| x.to_string())
                            .collect::<Vec<String>>()
                    })
                    .unwrap_or_default();

                Ok((
                    r.get::<_, i32>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i32>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, i32>(4)? != 0,
                    r.get::<_, i32>(5)? != 0,
                    footnote_ids,
                    r.get::<_, Option<String>>(7)?,
                ))
            })
            .ok()?;

        let mut items = Vec::new();
        let mut blocks = Vec::new();

        for row in rows.filter_map(|r| r.ok()) {
            let (
                position,
                item_type,
                verse_number,
                text,
                is_poem,
                is_words_of_jesus,
                footnote_ids,
                raw_json,
            ) = row;

            let resolved_text = text.clone().unwrap_or_default();
            let spans = extract_chapter_item_spans(
                raw_json.as_deref(),
                &resolved_text,
                is_words_of_jesus,
                is_poem,
            );

            let is_verse_like = matches!(item_type.as_str(), "verse" | "verses");
            let verses = if is_verse_like {
                build_reader_verses(
                    raw_json.as_deref(),
                    verse_number.unwrap_or(1),
                    &resolved_text,
                    &spans,
                    &footnote_ids,
                    is_words_of_jesus,
                    is_poem,
                    &timings_by_verse,
                )
            } else {
                Vec::new()
            };

            let item_woj = is_words_of_jesus
                || spans.iter().any(|s| s.formatting.is_words_of_jesus)
                || verses
                    .iter()
                    .any(|v| v.spans.iter().any(|s| s.formatting.is_words_of_jesus));

            items.push(GenericChapterItem {
                position,
                item_type: item_type.clone(),
                verse_number,
                text,
                is_poem,
                is_words_of_jesus: item_woj,
                footnote_ids: footnote_ids.clone(),
                spans: spans.clone(),
                raw_json,
            });

            blocks.push(ReaderBlock {
                position,
                item_type,
                verse_number,
                text: resolved_text,
                is_poem,
                is_words_of_jesus: item_woj,
                spans,
                verses,
                footnote_ids,
            });
        }

        let total_chapters: i32 = conn
            .query_row(
                "SELECT number_of_chapters FROM books WHERE translation_id = ?1 AND id = ?2",
                rusqlite::params![translation_id, book_id],
                |r| r.get(0),
            )
            .unwrap_or(0);

        Some(ReaderChapter {
            translation_id: translation_id.clone(),
            book_id: book_id.clone(),
            book_name,
            chapter_number: chapter,
            text_direction,
            items,
            blocks,
            footnotes,
            audio_timings,
            next_chapter: if chapter < total_chapters {
                Some(chapter + 1)
            } else {
                None
            },
            prev_chapter: if chapter > 1 { Some(chapter - 1) } else { None },
        })
    }

    pub fn get_verse(
        &self,
        translation_id: String,
        book_id: String,
        chapter: i32,
        verse: i32,
    ) -> Option<String> {
        let conn_arc = self
            .conn
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()?;
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());
        conn.query_row(
            "SELECT text FROM verses WHERE translation_id = ?1 AND book_id = ?2 AND chapter = ?3 AND verse = ?4",
            rusqlite::params![translation_id, book_id, chapter, verse],
            |r| r.get::<_, String>(0),
        )
        .ok()
    }

    // ── Entities (People, Places, Events, Groups) ────────────────

    pub fn get_person(&self, dataset_id: String, id: String) -> Option<PersonDetail> {
        let conn_arc = self
            .conn
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()?;
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let row = conn
            .query_row(
                "SELECT id, name, also_called, gender, birth_year, death_year, min_year, max_year, description
                 FROM people WHERE dataset_id = ?1 AND id = ?2",
                rusqlite::params![dataset_id, id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<i64>>(4)?,
                        r.get::<_, Option<i64>>(5)?,
                        r.get::<_, Option<i64>>(6)?,
                        r.get::<_, Option<i64>>(7)?,
                        r.get::<_, Option<String>>(8)?,
                    ))
                },
            )
            .ok()?;

        let also_called: Vec<String> = row
            .2
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();

        Some(PersonDetail {
            id: row.0.clone(),
            name: row.1,
            also_called,
            gender: row.3,
            birth_year: row.4,
            death_year: row.5,
            min_year: row.6,
            max_year: row.7,
            description: row.8,
            father: read_forward_refs(&conn, &dataset_id, "person", &id, &["father"]),
            mother: read_forward_refs(&conn, &dataset_id, "person", &id, &["mother"]),
            partners: read_forward_refs(&conn, &dataset_id, "person", &id, &["partner"]),
            children: read_forward_refs(&conn, &dataset_id, "person", &id, &["child"]),
            siblings: read_forward_refs(&conn, &dataset_id, "person", &id, &["sibling"]),
            half_siblings_same_father: read_forward_refs(
                &conn,
                &dataset_id,
                "person",
                &id,
                &["half_sibling_same_father"],
            ),
            half_siblings_same_mother: read_forward_refs(
                &conn,
                &dataset_id,
                "person",
                &id,
                &["half_sibling_same_mother"],
            ),
            member_of: read_forward_refs(&conn, &dataset_id, "person", &id, &["member_of"]),
            birth_place: read_forward_refs(&conn, &dataset_id, "person", &id, &["birth_place"]),
            death_place: read_forward_refs(&conn, &dataset_id, "person", &id, &["death_place"]),
            events: read_forward_refs(&conn, &dataset_id, "person", &id, &["participated_in"]),
            referenced_by_events: read_reverse_refs(
                &conn,
                &dataset_id,
                "person",
                &id,
                &["participant"],
            ),
            verses: read_verse_refs(&conn, &dataset_id, "person", &id),
        })
    }

    pub fn get_event(&self, dataset_id: String, id: String) -> Option<EventDetail> {
        let conn_arc = self
            .conn
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()?;
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let row = conn
            .query_row(
                "SELECT name, start_date, end_date, duration, description
                 FROM events WHERE dataset_id = ?1 AND id = ?2",
                rusqlite::params![dataset_id, id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .ok()?;

        Some(EventDetail {
            id: id.clone(),
            name: row.0,
            start_date: row.1,
            end_date: row.2,
            duration: row.3,
            description: row.4,
            participants: read_forward_refs(&conn, &dataset_id, "event", &id, &["participant"]),
            locations: read_forward_refs(&conn, &dataset_id, "event", &id, &["location"]),
            predecessor: read_forward_refs(&conn, &dataset_id, "event", &id, &["predecessor"])
                .into_iter()
                .next(),
            successor: read_forward_refs(&conn, &dataset_id, "event", &id, &["successor"])
                .into_iter()
                .next(),
            parent_event: read_forward_refs(&conn, &dataset_id, "event", &id, &["part_of"])
                .into_iter()
                .next(),
            sub_events: read_reverse_refs(&conn, &dataset_id, "event", &id, &["part_of"]),
            verses: read_verse_refs(&conn, &dataset_id, "event", &id),
        })
    }

    pub fn get_group(&self, dataset_id: String, id: String) -> Option<GroupDetail> {
        let conn_arc = self
            .conn
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()?;
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let row = conn
            .query_row(
                "SELECT name, description FROM people_groups WHERE dataset_id = ?1 AND id = ?2",
                rusqlite::params![dataset_id, id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .ok()?;

        Some(GroupDetail {
            id: id.clone(),
            name: row.0,
            description: row.1,
            members: read_forward_refs(&conn, &dataset_id, "group", &id, &["member"]),
            leaders: read_forward_refs(&conn, &dataset_id, "group", &id, &["leader"]),
            located_at: read_forward_refs(&conn, &dataset_id, "group", &id, &["located_at"]),
            verses: read_verse_refs(&conn, &dataset_id, "group", &id),
        })
    }

    pub fn get_place(&self, dataset_id: String, id: String) -> Option<PlaceDetail> {
        let conn_arc = self
            .conn
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()?;
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let row = conn
            .query_row(
                "SELECT name, kjv_name, esv_name, feature_type, latitude, longitude, description
                 FROM places WHERE dataset_id = ?1 AND id = ?2",
                rusqlite::params![dataset_id, id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Option<String>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<f64>>(4)?,
                        r.get::<_, Option<f64>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                    ))
                },
            )
            .ok()?;

        Some(PlaceDetail {
            id: id.clone(),
            name: row.0,
            kjv_name: row.1,
            esv_name: row.2,
            feature_type: row.3,
            latitude: row.4,
            longitude: row.5,
            description: row.6,
            events: read_forward_refs(&conn, &dataset_id, "place", &id, &["event_here"]),
            people_born_here: read_reverse_refs(&conn, &dataset_id, "place", &id, &["birth_place"]),
            people_died_here: read_reverse_refs(&conn, &dataset_id, "place", &id, &["death_place"]),
            people_groups_here: read_reverse_refs(
                &conn,
                &dataset_id,
                "place",
                &id,
                &["located_at"],
            ),
            verses: read_verse_refs(&conn, &dataset_id, "place", &id),
        })
    }

    pub fn list_people(
        &self,
        dataset_id: String,
        offset: i32,
        limit: i32,
        query: Option<String>,
    ) -> Vec<EntitySummary> {
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let (sql, params): (String, Vec<Box<dyn rusqlite::ToSql>>) = match query {
            Some(q) if !q.trim().is_empty() => (
                "SELECT id, name, is_proper_name, gender, NULL, (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='person' AND source_id=people.id)
                 FROM people WHERE dataset_id=?1 AND name LIKE ?2 ORDER BY name LIMIT ?3 OFFSET ?4".into(),
                vec![
                    Box::new(dataset_id.clone()),
                    Box::new(format!("%{}%", q)),
                    Box::new(limit),
                    Box::new(offset),
                ],
            ),
            _ => (
                "SELECT id, name, is_proper_name, gender, NULL, (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='person' AND source_id=people.id)
                 FROM people WHERE dataset_id=?1 ORDER BY name LIMIT ?2 OFFSET ?3".into(),
                vec![
                    Box::new(dataset_id.clone()),
                    Box::new(limit),
                    Box::new(offset),
                ],
            ),
        };
        let p: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match stmt.query_map(&p[..], |r| {
            Ok(EntitySummary {
                id: r.get(0)?,
                name: r.get(1)?,
                is_proper_name: r.get::<_, Option<i32>>(2)?.map(|v| v != 0),
                gender: r.get(3)?,
                start_date: r.get(4)?,
                number_of_references: r.get(5)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn list_events(
        &self,
        dataset_id: String,
        offset: i32,
        limit: i32,
        query: Option<String>,
    ) -> Vec<EntitySummary> {
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let (sql, params): (String, Vec<Box<dyn rusqlite::ToSql>>) = match query {
            Some(q) if !q.trim().is_empty() => (
                "SELECT id, name, NULL, NULL, start_date, (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='event' AND source_id=events.id)
                 FROM events WHERE dataset_id=?1 AND name LIKE ?2 ORDER BY name LIMIT ?3 OFFSET ?4".into(),
                vec![
                    Box::new(dataset_id.clone()),
                    Box::new(format!("%{}%", q)),
                    Box::new(limit),
                    Box::new(offset),
                ],
            ),
            _ => (
                "SELECT id, name, NULL, NULL, start_date, (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='event' AND source_id=events.id)
                 FROM events WHERE dataset_id=?1 ORDER BY name LIMIT ?2 OFFSET ?3".into(),
                vec![
                    Box::new(dataset_id.clone()),
                    Box::new(limit),
                    Box::new(offset),
                ],
            ),
        };
        let p: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match stmt.query_map(&p[..], |r| {
            Ok(EntitySummary {
                id: r.get(0)?,
                name: r.get(1)?,
                is_proper_name: None,
                gender: None,
                start_date: r.get(4)?,
                number_of_references: r.get(5)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn list_places(
        &self,
        dataset_id: String,
        offset: i32,
        limit: i32,
        query: Option<String>,
    ) -> Vec<EntitySummary> {
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let (sql, params): (String, Vec<Box<dyn rusqlite::ToSql>>) = match query {
            Some(q) if !q.trim().is_empty() => (
                "SELECT id, name, NULL, NULL, feature_type, (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='place' AND source_id=places.id)
                 FROM places WHERE dataset_id=?1 AND name LIKE ?2 ORDER BY name LIMIT ?3 OFFSET ?4".into(),
                vec![
                    Box::new(dataset_id.clone()),
                    Box::new(format!("%{}%", q)),
                    Box::new(limit),
                    Box::new(offset),
                ],
            ),
            _ => (
                "SELECT id, name, NULL, NULL, feature_type, (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='place' AND source_id=places.id)
                 FROM places WHERE dataset_id=?1 ORDER BY name LIMIT ?2 OFFSET ?3".into(),
                vec![
                    Box::new(dataset_id.clone()),
                    Box::new(limit),
                    Box::new(offset),
                ],
            ),
        };
        let p: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match stmt.query_map(&p[..], |r| {
            Ok(EntitySummary {
                id: r.get(0)?,
                name: r.get(1)?,
                is_proper_name: None,
                gender: None,
                start_date: r.get(4)?,
                number_of_references: r.get(5)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn list_groups(
        &self,
        dataset_id: String,
        offset: i32,
        limit: i32,
        query: Option<String>,
    ) -> Vec<EntitySummary> {
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let (sql, params): (String, Vec<Box<dyn rusqlite::ToSql>>) = match query {
            Some(q) if !q.trim().is_empty() => (
                "SELECT id, name, NULL, NULL, NULL, (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='group' AND source_id=people_groups.id)
                 FROM people_groups WHERE dataset_id=?1 AND name LIKE ?2 ORDER BY name LIMIT ?3 OFFSET ?4".into(),
                vec![
                    Box::new(dataset_id.clone()),
                    Box::new(format!("%{}%", q)),
                    Box::new(limit),
                    Box::new(offset),
                ],
            ),
            _ => (
                "SELECT id, name, NULL, NULL, NULL, (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='group' AND source_id=people_groups.id)
                 FROM people_groups WHERE dataset_id=?1 ORDER BY name LIMIT ?2 OFFSET ?3".into(),
                vec![
                    Box::new(dataset_id.clone()),
                    Box::new(limit),
                    Box::new(offset),
                ],
            ),
        };
        let p: Vec<&dyn rusqlite::ToSql> = params.iter().map(|b| b.as_ref()).collect();
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        match stmt.query_map(&p[..], |r| {
            Ok(EntitySummary {
                id: r.get(0)?,
                name: r.get(1)?,
                is_proper_name: None,
                gender: None,
                start_date: None,
                number_of_references: r.get(5)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    // ── Graph reads ──────────────────────────────────────────────

    pub fn get_verse_refs(&self, dataset_id: String, kind: String, id: String) -> Vec<VerseRange> {
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());
        read_verse_refs(&conn, &dataset_id, &kind, &id)
    }

    pub fn get_related(
        &self,
        dataset_id: String,
        kind: String,
        id: String,
        relation: Option<String>,
    ) -> Vec<EntityRef> {
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        match relation {
            Some(r) => read_forward_refs(&conn, &dataset_id, &kind, &id, &[r.as_str()]),
            None => {
                let mut stmt = match conn.prepare(
                    "SELECT target_kind, target_id, COALESCE(target_name, target_id)
                     FROM entity_refs WHERE dataset_id = ?1 AND source_kind = ?2 AND source_id = ?3
                     ORDER BY relation, target_name",
                ) {
                    Ok(s) => s,
                    Err(_) => return Vec::new(),
                };
                match stmt.query_map(rusqlite::params![dataset_id, kind, id], |row| {
                    Ok(EntityRef {
                        kind: row.get(0)?,
                        id: row.get(1)?,
                        name: row.get(2)?,
                    })
                }) {
                    Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
                    Err(_) => Vec::new(),
                }
            }
        }
    }

    pub fn get_entities_for_verse(
        &self,
        dataset_id: String,
        book: String,
        chapter: i32,
        verse: i32,
    ) -> Vec<EntityRef> {
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let mut stmt = match conn.prepare(
            "SELECT source_kind, source_id,
                    COALESCE((SELECT name FROM people WHERE id = source_id AND dataset_id = ?1),
                             (SELECT name FROM events WHERE id = source_id AND dataset_id = ?1),
                             (SELECT name FROM places WHERE id = source_id AND dataset_id = ?1),
                             (SELECT name FROM people_groups WHERE id = source_id AND dataset_id = ?1),
                             source_id)
             FROM entity_verse_refs
             WHERE dataset_id = ?1 AND book = ?2 AND chapter = ?3
               AND ?4 >= verse AND (?4 <= end_verse OR end_verse IS NULL)
             ORDER BY CASE source_kind
                 WHEN 'person' THEN 1 WHEN 'event' THEN 2 WHEN 'place' THEN 3 WHEN 'group' THEN 4 ELSE 5 END, source_id",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        match stmt.query_map(rusqlite::params![dataset_id, book, chapter, verse], |r| {
            Ok(EntityRef {
                kind: r.get(0)?,
                id: r.get(1)?,
                name: r.get(2)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn search_entities(&self, dataset_id: String, query: String) -> Vec<EntitySummary> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let pattern = format!("%{}%", query);
        let mut stmt = match conn.prepare(
            "SELECT id, name, NULL, NULL, NULL, 0 FROM people WHERE dataset_id = ?1 AND name LIKE ?2
             UNION ALL SELECT id, name, NULL, NULL, start_date, 0 FROM events WHERE dataset_id = ?1 AND name LIKE ?2
             UNION ALL SELECT id, name, NULL, NULL, NULL, 0 FROM places WHERE dataset_id = ?1 AND name LIKE ?2
             UNION ALL SELECT id, name, NULL, NULL, NULL, 0 FROM people_groups WHERE dataset_id = ?1 AND name LIKE ?2
             ORDER BY 2 LIMIT 200",
        ) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        match stmt.query_map(rusqlite::params![dataset_id, pattern], |r| {
            Ok(EntitySummary {
                id: r.get(0)?,
                name: r.get(1)?,
                is_proper_name: None,
                gender: None,
                start_date: r.get(4)?,
                number_of_references: r.get(5)?,
            })
        }) {
            Ok(rows) => rows.filter_map(|x| x.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    // ── Installed state ─────────────────────────────────────────

    pub fn is_entity_set_installed(&self, dataset_id: String, kind: String) -> bool {
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => return false,
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());
        conn.query_row(
            "SELECT installed FROM entity_install_state WHERE dataset_id = ?1 AND kind = ?2",
            rusqlite::params![dataset_id, kind],
            |r| r.get::<_, i32>(0),
        )
        .map(|v| v == 1)
        .unwrap_or(false)
    }

    pub fn is_commentary_installed(&self, commentary_id: String) -> Result<bool, XBibleError> {
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => {
                return Err(XBibleError::Generic {
                    error_message: "reader has no connection — call setDbPath first".into(),
                });
            }
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let in_catalog = conn
            .query_row(
                "SELECT 1 FROM commentaries WHERE id = ?1",
                rusqlite::params![commentary_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|e| XBibleError::Generic {
                error_message: format!(
                    "is_commentary_installed({commentary_id}) catalog check failed: {e}"
                ),
            })?
            .is_some();

        if !in_catalog {
            return Err(XBibleError::Generic {
                error_message: format!(
                    "commentary {commentary_id} not in catalog — call refresh() first"
                ),
            });
        }

        let installed: i32 = conn
            .query_row(
                "SELECT installed FROM commentaries WHERE id = ?1",
                rusqlite::params![commentary_id],
                |r| r.get(0),
            )
            .map_err(|e| XBibleError::Generic {
                error_message: format!(
                    "is_commentary_installed({commentary_id}) query failed: {e}"
                ),
            })?;

        Ok(installed == 1)
    }

    pub fn read_commentary_chapter(
        &self,
        commentary_id: String,
        book_id: String,
        chapter: i32,
    ) -> Result<Option<CommentaryChapter>, XBibleError> {
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => {
                return Err(XBibleError::Generic {
                    error_message: "reader has no connection — call setDbPath first".into(),
                });
            }
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let book_row = conn
            .query_row(
                "SELECT name, number_of_chapters FROM commentary_books
                 WHERE commentary_id = ?1 AND id = ?2",
                rusqlite::params![commentary_id, book_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i32>(1)?)),
            )
            .optional()
            .map_err(|e| XBibleError::Generic {
                error_message: format!(
                    "read_commentary_chapter({commentary_id}, {book_id}) book lookup failed: {e}"
                ),
            })?;

        let Some((book_name, total_chapters)) = book_row else {
            return Ok(None);
        };

        let mut stmt = conn
            .prepare(
                "SELECT verse_number, text FROM commentary_chapters
                 WHERE commentary_id = ?1 AND book_id = ?2 AND chapter = ?3
                 ORDER BY position",
            )
            .map_err(|e| XBibleError::Generic {
                error_message: format!(
                    "read_commentary_chapter({commentary_id}, {book_id} {chapter}) prepare failed: {e}"
                ),
            })?;

        let rows = stmt
            .query_map(
                rusqlite::params![commentary_id, book_id, chapter],
                |r| {
                    Ok(CommentaryBlock {
                        verse_number: r.get(0)?,
                        text: r.get(1)?,
                    })
                },
            )
            .map_err(|e| XBibleError::Generic {
                error_message: format!(
                    "read_commentary_chapter({commentary_id}, {book_id} {chapter}) query failed: {e}"
                ),
            })?;

        let blocks: Vec<CommentaryBlock> = rows
            .filter_map(|r| match r {
                Ok(v) => Some(v),
                Err(e) => {
                    log::error!(
                        "read_commentary_chapter({commentary_id}, {book_id} {chapter}) row failed: {e}"
                    );
                    None
                }
            })
            .collect();

        if blocks.is_empty() {
            return Ok(None);
        }

        Ok(Some(CommentaryChapter {
            commentary_id,
            book_id,
            book_name,
            chapter_number: chapter,
            blocks,
            next_chapter: if chapter < total_chapters {
                Some(chapter + 1)
            } else {
                None
            },
            prev_chapter: if chapter > 1 { Some(chapter - 1) } else { None },
        }))
    }

    pub fn is_installed(&self, translation_id: String) -> Result<bool, XBibleError> {
        log::info!("is_installed called with id={:?}", translation_id);

        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => {
                log::warn!("is_installed({}): reader has no connection", translation_id);
                return Err(XBibleError::Generic {
                    error_message: "reader has no connection — call setDbPath first".into(),
                });
            }
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let in_catalog = conn
            .query_row(
                "SELECT 1 FROM translations WHERE id = ?1",
                rusqlite::params![translation_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|e| XBibleError::Generic {
                error_message: format!("is_installed({translation_id}) catalog check failed: {e}"),
            })?
            .is_some();

        if !in_catalog {
            return Err(XBibleError::Generic {
                error_message: format!(
                    "translation {translation_id} not in catalog — call refresh() first"
                ),
            });
        }

        let installed: i32 = conn
            .query_row(
                "SELECT installed FROM translations WHERE id = ?1",
                rusqlite::params![translation_id],
                |r| r.get(0),
            )
            .map_err(|e| XBibleError::Generic {
                error_message: format!("is_installed({translation_id}) query failed: {e}"),
            })?;

        Ok(installed == 1)
    }
}

// ── Internal API — only the engine calls this ─────────────────
impl XBibleReader {
    /// Attach the shared SQLite connection. Not exposed to UniFFI.
    pub(crate) fn attach_connection(&self, conn: Arc<Mutex<Connection>>) {
        *self.conn.write().unwrap_or_else(|p| p.into_inner()) = Some(conn);
    }
}
