use rusqlite::{Connection, OptionalExtension};
use std::sync::{Arc, Mutex, RwLock};

use crate::engines::freebible_engine::client::{EntitySummary, FreeBibleBook, FreeBibleFootnote};
use crate::engines::freebible_engine::error::FreeBibleError;

// ── UniFFI Data Models ─────────────────────────────────────────

#[derive(Debug, Clone, uniffi::Record)]
pub struct ReaderWord {
    pub text: String,
    pub is_words_of_jesus: bool,
    pub strongs: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ReaderVerse {
    pub number: i32,
    pub text: String,
    pub words: Vec<ReaderWord>,
    pub footnote_ids: Vec<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ReaderBlock {
    pub item_type: String,
    pub verse_number: Option<i32>,
    pub text: String,
    pub is_poem: bool,
    pub is_words_of_jesus: bool,
    pub verses: Vec<ReaderVerse>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct ReaderChapter {
    pub translation_id: String,
    pub book_id: String,
    pub book_name: String,
    pub chapter_number: i32,
    pub blocks: Vec<ReaderBlock>,
    pub footnotes: Vec<FreeBibleFootnote>,
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

#[derive(Debug, Clone, uniffi::Record)]
pub struct InstalledTranslation {
    pub id: String,
    pub name: String,
    pub english_name: String,
    pub short_name: String,
    pub language: String,
    pub language_name: Option<String>,
    pub text_direction: String,
    pub book_count: i32,
    pub chapter_count: i32,
    pub verse_count: i32,
}

// Entity Models (Moved from store_api)
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

// ── DB Helpers ───────────────────────────────────────────────────

// Helper functions to read relations
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
         WHERE dataset_id = ?1 AND source_kind = ?2 AND source_id = ?3 ORDER BY book, chapter, verse"
    ) { Ok(s) => s, Err(_) => return Vec::new() };
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

fn parse_verse_words(raw_json: &str, fallback_text: String) -> Vec<ReaderVerse> {
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(raw_json);
    if let Ok(json) = parsed {
        if let Some(content_arr) = json.get("content").and_then(|c| c.as_array()) {
            let mut words = Vec::new();
            for piece in content_arr {
                if let Some(s) = piece.as_str() {
                    words.push(ReaderWord {
                        text: s.to_string(),
                        is_words_of_jesus: false,
                        strongs: None,
                    });
                } else if piece.is_object() {
                    let text = piece
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    let is_red = piece
                        .get("wordsOfJesus")
                        .and_then(|b| b.as_bool())
                        .unwrap_or(false);
                    let strongs = piece
                        .get("strong")
                        .and_then(|s| s.as_str())
                        .map(|s| s.to_string());
                    words.push(ReaderWord {
                        text,
                        is_words_of_jesus: is_red,
                        strongs,
                    });
                }
            }
            let number = json.get("number").and_then(|n| n.as_i64()).unwrap_or(1) as i32;
            return vec![ReaderVerse {
                number,
                text: fallback_text,
                words,
                footnote_ids: vec![],
            }];
        }
    }
    vec![ReaderVerse {
        number: 1,
        text: fallback_text.clone(),
        words: vec![ReaderWord {
            text: fallback_text,
            is_words_of_jesus: false,
            strongs: None,
        }],
        footnote_ids: vec![],
    }]
}

// ── Reader UniFFI Object ─────────────────────────────────────────

#[derive(Debug, uniffi::Object)]
pub struct FreeBibleReader {
    conn: RwLock<Option<Arc<Mutex<Connection>>>>,
}

#[uniffi::export]
impl FreeBibleReader {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            conn: RwLock::new(None),
        })
    }

    pub fn set_db_path(&self, path: String) -> Result<(), FreeBibleError> {
        let conn = crate::db::open(&path).map_err(|e| FreeBibleError::Generic {
            error_message: e.to_string(),
        })?;
        let mut lock = self.conn.write().unwrap();
        *lock = Some(Arc::new(Mutex::new(conn)));
        Ok(())
    }

    // ── Translations & Books ─────────────────────────────────────

    pub fn get_installed_translations(&self) -> Vec<InstalledTranslation> {
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => {
                log::warn!("get_installed_translations: reader has no connection");
                return Vec::new();
            }
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        let mut stmt = match conn.prepare(
            "SELECT id, name, english_name, short_name, language, language_name,
                text_direction, number_of_books, total_number_of_chapters, total_number_of_verses
         FROM translations WHERE installed = 1 ORDER BY name",
        ) {
            Ok(s) => s,
            Err(e) => {
                log::error!("get_installed_translations prepare failed: {}", e);
                return Vec::new();
            }
        };

        let rows = stmt.query_map([], |r| {
            Ok(InstalledTranslation {
                id: r.get(0)?,
                name: r.get(1)?,
                english_name: r.get(2)?,
                short_name: r.get(3)?,
                language: r.get(4)?,
                language_name: r.get(5)?,
                text_direction: r.get(6)?,
                book_count: r.get(7)?,
                chapter_count: r.get(8)?,
                verse_count: r.get(9)?,
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

    pub fn get_books(&self, translation_id: String) -> Vec<FreeBibleBook> {
        let conn_arc = match self.conn.read().unwrap().clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap();

        let mut stmt = match conn.prepare(
            "SELECT b.id, b.name, b.number_of_chapters, m.common_name, m.title, m.book_order, m.audio_links_json, m.raw_json
             FROM books b LEFT JOIN book_metadata m ON m.translation_id = b.translation_id AND m.book_id = b.id
             WHERE b.translation_id = ?1 ORDER BY COALESCE(m.book_order, 999999), b.id"
        ) { Ok(s) => s, Err(_) => return Vec::new() };

        let rows = stmt.query_map(rusqlite::params![translation_id], |r| {
            Ok(FreeBibleBook {
                id: r.get(0)?,
                name: r.get(1)?,
                number_of_chapters: r.get(2)?,
                common_name: r.get(3)?,
                title: r.get(4)?,
                book_order: r.get(5)?,
                audio_links_json: r.get(6)?,
                raw_json: r.get(7)?,
            })
        });

        match rows {
            Ok(r) => r.filter_map(|x| x.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    // ── Chapter Reading ──────────────────────────────────────────

    pub fn read_chapter(
        &self,
        translation_id: String,
        book_id: String,
        chapter: i32,
    ) -> Option<ReaderChapter> {
        let conn_arc = self.conn.read().unwrap().clone()?;
        let conn = conn_arc.lock().unwrap();

        let book_name: String = conn
            .query_row(
                "SELECT name FROM books WHERE translation_id = ?1 AND id = ?2",
                rusqlite::params![translation_id, book_id],
                |r| r.get(0),
            )
            .unwrap_or_else(|_| book_id.clone());

        let mut f_stmt = conn.prepare(
            "SELECT note_id, caller, text, reference, raw_json FROM footnotes WHERE translation_id = ?1 AND book_id = ?2 AND chapter = ?3"
        ).ok()?;

        let footnotes: Vec<FreeBibleFootnote> = f_stmt
            .query_map(rusqlite::params![translation_id, book_id, chapter], |r| {
                Ok(FreeBibleFootnote {
                    note_id: r.get(0)?,
                    caller: r.get(1)?,
                    text: r.get(2)?,
                    reference: r.get(3)?,
                    raw_json: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                })
            })
            .ok()?
            .filter_map(|r| r.ok())
            .collect();

        let mut stmt = conn.prepare(
            "SELECT position, item_type, verse_number, text, poem, words_of_jesus, footnote_ids, raw_json
             FROM chapter_items WHERE translation_id = ?1 AND book_id = ?2 AND chapter = ?3 ORDER BY position"
        ).ok()?;

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

        let mut blocks = Vec::new();
        for row in rows.filter_map(|r| r.ok()) {
            let (item_type, verse_number, text, is_poem, is_words_of_jesus, footnote_ids, raw_json) =
                row;
            let mut verses = Vec::new();
            if item_type == "verse" {
                if let Some(raw) = raw_json {
                    verses = parse_verse_words(&raw, text.clone().unwrap_or_default());
                } else {
                    verses.push(ReaderVerse {
                        number: verse_number.unwrap_or(0),
                        text: text.clone().unwrap_or_default(),
                        words: vec![],
                        footnote_ids: footnote_ids.clone(),
                    });
                }
            }
            blocks.push(ReaderBlock {
                item_type,
                verse_number,
                text: text.unwrap_or_default(),
                is_poem,
                is_words_of_jesus,
                verses,
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
            blocks,
            footnotes,
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
        let conn_arc = self.conn.read().unwrap().clone()?;
        let conn = conn_arc.lock().unwrap();
        conn.query_row(
            "SELECT text FROM verses WHERE translation_id = ?1 AND book_id = ?2 AND chapter = ?3 AND verse = ?4",
            rusqlite::params![translation_id, book_id, chapter, verse], |r| r.get::<_, String>(0)
        ).ok()
    }

    // ── Entities (People, Places, Events) ────────────────────────

    pub fn get_person(&self, dataset_id: String, id: String) -> Option<PersonDetail> {
        let conn_arc = self.conn.read().unwrap().clone()?;
        let conn = conn_arc.lock().unwrap();

        let row = conn.query_row(
            "SELECT id, name, also_called, gender, birth_year, death_year, min_year, max_year, description
             FROM people WHERE dataset_id = ?1 AND id = ?2", rusqlite::params![dataset_id, id],
            |r| Ok((
                r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<i64>>(4)?, r.get::<_, Option<i64>>(5)?, r.get::<_, Option<i64>>(6)?, r.get::<_, Option<i64>>(7)?, r.get::<_, Option<String>>(8)?,
            )),
        ).ok()?;

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
        let conn_arc = self.conn.read().unwrap().clone()?;
        let conn = conn_arc.lock().unwrap();

        let row = conn.query_row(
            "SELECT name, start_date, end_date, duration, description FROM events WHERE dataset_id = ?1 AND id = ?2",
            rusqlite::params![dataset_id, id],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?, r.get::<_, Option<String>>(4)?)),
        ).ok()?;

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
        let conn_arc = self.conn.read().unwrap().clone()?;
        let conn = conn_arc.lock().unwrap();

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
        let conn_arc = self.conn.read().unwrap().clone()?;
        let conn = conn_arc.lock().unwrap();

        let row = conn.query_row(
            "SELECT name, kjv_name, esv_name, feature_type, latitude, longitude, description FROM places WHERE dataset_id = ?1 AND id = ?2",
            rusqlite::params![dataset_id, id],
            |r| Ok((
                r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<f64>>(4)?, r.get::<_, Option<f64>>(5)?, r.get::<_, Option<String>>(6)?,
            )),
        ).ok()?;

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
        let conn_arc = match self.conn.read().unwrap().clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap();

        let (sql, params): (String, Vec<Box<dyn rusqlite::ToSql>>) = match query {
            Some(q) if !q.trim().is_empty() => (
                "SELECT id, name, is_proper_name, gender, NULL, (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='person' AND source_id=people.id)
                 FROM people WHERE dataset_id=?1 AND name LIKE ?2 ORDER BY name LIMIT ?3 OFFSET ?4".into(),
                vec![Box::new(dataset_id.clone()), Box::new(format!("%{}%", q)), Box::new(limit), Box::new(offset)],
            ),
            _ => (
                "SELECT id, name, is_proper_name, gender, NULL, (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='person' AND source_id=people.id)
                 FROM people WHERE dataset_id=?1 ORDER BY name LIMIT ?2 OFFSET ?3".into(),
                vec![Box::new(dataset_id.clone()), Box::new(limit), Box::new(offset)],
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
        let conn_arc = match self.conn.read().unwrap().clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap();

        let (sql, params): (String, Vec<Box<dyn rusqlite::ToSql>>) = match query {
            Some(q) if !q.trim().is_empty() => (
                "SELECT id, name, NULL, NULL, start_date, (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='event' AND source_id=events.id)
                 FROM events WHERE dataset_id=?1 AND name LIKE ?2 ORDER BY name LIMIT ?3 OFFSET ?4".into(),
                vec![Box::new(dataset_id.clone()), Box::new(format!("%{}%", q)), Box::new(limit), Box::new(offset)],
            ),
            _ => (
                "SELECT id, name, NULL, NULL, start_date, (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='event' AND source_id=events.id)
                 FROM events WHERE dataset_id=?1 ORDER BY name LIMIT ?2 OFFSET ?3".into(),
                vec![Box::new(dataset_id.clone()), Box::new(limit), Box::new(offset)],
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
        let conn_arc = match self.conn.read().unwrap().clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap();

        let (sql, params): (String, Vec<Box<dyn rusqlite::ToSql>>) = match query {
            Some(q) if !q.trim().is_empty() => (
                "SELECT id, name, NULL, NULL, feature_type, (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='place' AND source_id=places.id)
                 FROM places WHERE dataset_id=?1 AND name LIKE ?2 ORDER BY name LIMIT ?3 OFFSET ?4".into(),
                vec![Box::new(dataset_id.clone()), Box::new(format!("%{}%", q)), Box::new(limit), Box::new(offset)],
            ),
            _ => (
                "SELECT id, name, NULL, NULL, feature_type, (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='place' AND source_id=places.id)
                 FROM places WHERE dataset_id=?1 ORDER BY name LIMIT ?2 OFFSET ?3".into(),
                vec![Box::new(dataset_id.clone()), Box::new(limit), Box::new(offset)],
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
        let conn_arc = match self.conn.read().unwrap().clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap();

        let (sql, params): (String, Vec<Box<dyn rusqlite::ToSql>>) = match query {
            Some(q) if !q.trim().is_empty() => (
                "SELECT id, name, NULL, NULL, NULL, (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='group' AND source_id=people_groups.id)
                 FROM people_groups WHERE dataset_id=?1 AND name LIKE ?2 ORDER BY name LIMIT ?3 OFFSET ?4".into(),
                vec![Box::new(dataset_id.clone()), Box::new(format!("%{}%", q)), Box::new(limit), Box::new(offset)],
            ),
            _ => (
                "SELECT id, name, NULL, NULL, NULL, (SELECT COUNT(*) FROM entity_refs WHERE dataset_id=?1 AND source_kind='group' AND source_id=people_groups.id)
                 FROM people_groups WHERE dataset_id=?1 ORDER BY name LIMIT ?2 OFFSET ?3".into(),
                vec![Box::new(dataset_id.clone()), Box::new(limit), Box::new(offset)],
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
        let conn_arc = match self.conn.read().unwrap().clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap();
        read_verse_refs(&conn, &dataset_id, &kind, &id)
    }

    pub fn get_related(
        &self,
        dataset_id: String,
        kind: String,
        id: String,
        relation: Option<String>,
    ) -> Vec<EntityRef> {
        let conn_arc = match self.conn.read().unwrap().clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap();

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
        let conn_arc = match self.conn.read().unwrap().clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap();

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
        ) { Ok(s) => s, Err(_) => return Vec::new() };

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
        let conn_arc = match self.conn.read().unwrap().clone() {
            Some(c) => c,
            None => return Vec::new(),
        };
        let conn = conn_arc.lock().unwrap();

        let pattern = format!("%{}%", query);
        let mut stmt = match conn.prepare(
            "SELECT id, name, NULL, NULL, NULL, 0 FROM people WHERE dataset_id = ?1 AND name LIKE ?2
             UNION ALL SELECT id, name, NULL, NULL, start_date, 0 FROM events WHERE dataset_id = ?1 AND name LIKE ?2
             UNION ALL SELECT id, name, NULL, NULL, NULL, 0 FROM places WHERE dataset_id = ?1 AND name LIKE ?2
             UNION ALL SELECT id, name, NULL, NULL, NULL, 0 FROM people_groups WHERE dataset_id = ?1 AND name LIKE ?2
             ORDER BY 2 LIMIT 200",
        ) { Ok(s) => s, Err(_) => return Vec::new() };

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

    // ── Installed state (query for library screen) ───────────────

    pub fn is_entity_set_installed(&self, dataset_id: String, kind: String) -> bool {
        let conn_arc = match self.conn.read().unwrap().clone() {
            Some(c) => c,
            None => return false,
        };
        let conn = conn_arc.lock().unwrap();
        conn.query_row(
            "SELECT installed FROM entity_install_state WHERE dataset_id = ?1 AND kind = ?2",
            rusqlite::params![dataset_id, kind],
            |r| r.get::<_, i32>(0),
        )
        .map(|v| v == 1)
        .unwrap_or(false)
    }

    pub fn is_commentary_installed(&self, commentary_id: String) -> Result<bool, FreeBibleError> {
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => {
                return Err(FreeBibleError::Generic {
                    error_message: "reader has no connection — call setDbPath first".into(),
                });
            }
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        // Same catalog-check pattern as `is_installed`: a typo like
        // "adam-clark" should fail loudly, not read as "not installed".
        let in_catalog = conn
            .query_row(
                "SELECT 1 FROM commentaries WHERE id = ?1",
                rusqlite::params![commentary_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|e| FreeBibleError::Generic {
                error_message: format!(
                    "is_commentary_installed({commentary_id}) catalog check failed: {e}"
                ),
            })?
            .is_some();

        if !in_catalog {
            return Err(FreeBibleError::Generic {
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
            .map_err(|e| FreeBibleError::Generic {
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
    ) -> Result<Option<CommentaryChapter>, FreeBibleError> {
        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => {
                return Err(FreeBibleError::Generic {
                    error_message: "reader has no connection — call setDbPath first".into(),
                });
            }
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        // Book name + total-chapters lookup. If the book isn't in
        // `commentary_books`, the commentary has nothing for it — return
        // Ok(None), which the caller renders as "no commentary here".
        let book_row = conn
            .query_row(
                "SELECT name, number_of_chapters FROM commentary_books
             WHERE commentary_id = ?1 AND id = ?2",
                rusqlite::params![commentary_id, book_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i32>(1)?)),
            )
            .optional()
            .map_err(|e| FreeBibleError::Generic {
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
        .map_err(|e| FreeBibleError::Generic {
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
        .map_err(|e| FreeBibleError::Generic {
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

        // No rows for this chapter: the book exists but this chapter has
        // no commentary. Same contract as a missing book — Ok(None).
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

    pub fn is_installed(&self, translation_id: String) -> Result<bool, FreeBibleError> {
        log::info!("is_installed called with id={:?}", translation_id);

        let conn_arc = match self.conn.read().unwrap_or_else(|p| p.into_inner()).clone() {
            Some(c) => c,
            None => {
                log::warn!("is_installed({}): reader has no connection", translation_id);
                return Err(FreeBibleError::Generic {
                    error_message: "reader has no connection — call setDbPath first".into(),
                });
            }
        };
        let conn = conn_arc.lock().unwrap_or_else(|p| p.into_inner());

        // First: is the id even in the catalog? A typo like "BSBcl"
        // would otherwise look identical to "not installed".
        let in_catalog = conn
            .query_row(
                "SELECT 1 FROM translations WHERE id = ?1",
                rusqlite::params![translation_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|e| FreeBibleError::Generic {
                error_message: format!("is_installed({translation_id}) catalog check failed: {e}"),
            })?
            .is_some();

        if !in_catalog {
            return Err(FreeBibleError::Generic {
                error_message: format!(
                    "translation {translation_id} not in catalog — call refresh() first"
                ),
            });
        }

        // Second: read the installed flag. The row is known to exist,
        // so a QueryReturnedNoRows here would be a race with an uninstall
        // that we can't meaningfully recover from.
        let installed: i32 = conn
            .query_row(
                "SELECT installed FROM translations WHERE id = ?1",
                rusqlite::params![translation_id],
                |r| r.get(0),
            )
            .map_err(|e| FreeBibleError::Generic {
                error_message: format!("is_installed({translation_id}) query failed: {e}"),
            })?;

        Ok(installed == 1)
    }
}

// ── Internal API — only the engine calls this ─────────────────
impl FreeBibleReader {
    /// Attach the shared SQLite connection. Called once by
    /// `FreeBibleEngine::set_db_path`. Not exposed to UniFFI.
    pub(crate) fn attach_connection(&self, conn: Arc<Mutex<Connection>>) {
        *self.conn.write().unwrap_or_else(|p| p.into_inner()) = Some(conn);
    }
}
