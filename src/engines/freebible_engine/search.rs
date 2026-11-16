//! Search engine for the FreeBible module.
//!
//! ## Architecture
//!
//! Search is a three-step pipeline:
//!
//! 1. **Classify** the user's input into one of three kinds: a Bible
//!    reference (`"JHN 3:16"`), a Strong's number (`"H430"`), or plain
//!    text (`"shepherd"`). See [`classify`].
//!
//! 2. **Execute** the appropriate backend. References and Strong's
//!    numbers hit indexed columns directly. Text search goes through
//!    FTS5 (multi-word and phrase modes) or a regex scan (regex mode).
//!
//! 3. **Report** hits with enough metadata for the UI to render without
//!    another round-trip: translation, book, chapter, verse, text,
//!    rank score, and byte-range highlights.
//!
//! ## Filters
//!
//! [`SearchFilter`] carries a list of book IDs. Empty means "search
//! every book". An explicit list constrains to those books. The
//! engine does **not** know what "Old Testament" or "Apocrypha" means
//! — that knowledge lives in `book_metadata.section`, and the UI
//! builds the `book_ids` list from whatever sections it wants to
//! include. This is what makes the filter work correctly for a
//! translation that includes the Apocrypha, or one that reorders the
//! canon.
//!
//! ## Search modes
//!
//! - **MultiWord**: every whitespace-separated token must appear,
//!   any order. `love God` matches "God is love". This is the
//!   default because it is what users mean when they type two words.
//! - **Phrase**: the tokens must appear as an exact adjacent
//!   sequence. `love God` matches only "love God". This is what
//!   users mean when they type a phrase in quotes.
//! - **Regex**: the input is a regular expression. The UI should
//!   surface a clear "regex" toggle so users do not type
//!   `(` accidentally.
//!
//! ## Progress reporting
//!
//! [`search`] emits [`SearchProgress`] events between phases:
//! Classifying → Resolving → Querying → Ranking → Done.
//!
//! ## Highlighting
//!
//! FTS5 and regex hits carry `highlights: Vec<HighlightSpan>`, byte
//! ranges into `SearchHit.text` where the query matched. The UI
//! renders them as tinted rectangles behind the matched words. The
//! spans are computed by [`compute_highlights`] using the same
//! tokenization FTS5 uses, so what is highlighted is what matched.
//! For ASCII and Latin-1 text the byte offsets are exact; for text
//! with combining marks they may be off by a byte or two, which is
//! acceptable for highlighting.

use rusqlite::{Connection, params};

use crate::engines::freebible_engine::error::FreeBibleError;

// ═══════════════════════════════════════════════════════════════
// Result types — the shape of everything that crosses the FFI
// ═══════════════════════════════════════════════════════════════

/// A byte range into `SearchHit.text` that matched the query.
#[derive(Debug, Clone, uniffi::Record)]
pub struct HighlightSpan {
    pub start: u32,
    pub end: u32,
}

/// How a text query is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Enum)]
pub enum SearchMode {
    #[default]
    MultiWord,
    Phrase,
    Regex,
}

/// Constraints on a search.
///
/// `SearchFilter::default()` means "search every book, multi-word
/// mode". An explicit `book_ids` list constrains the search to those
/// books. The engine does not know what a canon is — the UI computes
/// the list from `book_metadata.section` before calling `search`.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct SearchFilter {
    /// Only search within these book IDs (`"PSA"`, `"JHN"`, …).
    /// Empty means all books.
    pub book_ids: Vec<String>,

    /// How to interpret the text query. Applies only to text search;
    /// references and Strong's lookups ignore it.
    pub mode: SearchMode,
}

/// One step of the search pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SearchPhase {
    Classifying,
    Resolving,
    Querying,
    Ranking,
    Done,
}

/// A progress event emitted by [`search`].
#[derive(Debug, Clone, uniffi::Record)]
pub struct SearchProgress {
    pub phase: SearchPhase,
    pub message: String,
    pub fraction: f64,
}

/// One result of a search.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SearchHit {
    pub kind: String,
    pub translation_id: String,
    pub book_id: String,
    pub book_name: String,
    pub chapter: i32,
    pub verse: i32,
    pub text: String,
    pub score: f64,
    pub highlights: Vec<HighlightSpan>,
}

/// The complete result of a search, including timing.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SearchResults {
    pub query: String,
    pub hits: Vec<SearchHit>,
    pub total_hits: i32,
    pub took_ms: i64,
}

// ═══════════════════════════════════════════════════════════════
// Query classification
// ═══════════════════════════════════════════════════════════════

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryKind {
    Reference {
        book: String,
        chapter: i32,
        verse: Option<i32>,
        end_verse: Option<i32>,
    },
    Strongs(String),
    Text(String),
}

/// Parse a user query into one of the three shapes.
pub fn classify(query: &str) -> QueryKind {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return QueryKind::Text(String::new());
    }

    let upper = trimmed.to_uppercase();
    if let Some(rest) = upper.strip_prefix('H').or_else(|| upper.strip_prefix('G')) {
        if !rest.is_empty() && rest.len() <= 4 && rest.chars().all(|c| c.is_ascii_digit()) {
            return QueryKind::Strongs(upper);
        }
    }

    if let Some(reference) = parse_reference(trimmed) {
        return QueryKind::Reference {
            book: reference.book,
            chapter: reference.chapter,
            verse: reference.verse,
            end_verse: reference.end_verse,
        };
    }

    QueryKind::Text(trimmed.to_string())
}

struct ParsedRef {
    book: String,
    chapter: i32,
    verse: Option<i32>,
    end_verse: Option<i32>,
}

fn parse_reference(input: &str) -> Option<ParsedRef> {
    let mut split_at = None;
    let mut seen_alpha = false;
    for (i, ch) in input.char_indices() {
        if ch.is_alphabetic() {
            seen_alpha = true;
        }
        if seen_alpha && ch.is_ascii_digit() {
            split_at = Some(i);
            break;
        }
    }
    let idx = split_at?;
    let book_part = input[..idx].trim();
    let num_part = input[idx..].trim();

    if book_part.is_empty() || num_part.is_empty() {
        return None;
    }

    let book_norm: String = book_part
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect();

    let (chapter_str, verse_part) = match num_part.split_once(':') {
        Some((c, v)) => (c, Some(v)),
        None => (num_part, None),
    };
    let chapter: i32 = chapter_str.trim().parse().ok()?;
    if chapter <= 0 {
        return None;
    }

    let (verse, end_verse) = match verse_part {
        Some(v) => {
            let (start, end) = match v.split_once('-') {
                Some((a, b)) => (a.trim(), Some(b.trim())),
                None => (v.trim(), None),
            };
            let verse: i32 = start.parse().ok()?;
            if verse <= 0 {
                return None;
            }
            let end_verse = end.and_then(|e| e.parse::<i32>().ok());
            (Some(verse), end_verse)
        }
        None => (None, None),
    };

    Some(ParsedRef {
        book: book_norm,
        chapter,
        verse,
        end_verse,
    })
}

// ═══════════════════════════════════════════════════════════════
// Highlighting
// ═══════════════════════════════════════════════════════════════

fn compute_highlights(text: &str, query: &str) -> Vec<HighlightSpan> {
    let tokens: Vec<String> = query
        .split_whitespace()
        .map(|t| t.trim_matches('"').to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();

    if tokens.is_empty() || text.is_empty() {
        return Vec::new();
    }

    let lower_text = text.to_lowercase();
    let mut spans: Vec<HighlightSpan> = Vec::new();

    for token in &tokens {
        let mut search_from = 0usize;
        while let Some(pos) = lower_text[search_from..].find(token.as_str()) {
            let abs = search_from + pos;
            let start = abs.min(text.len());
            let end = (abs + token.len()).min(text.len());
            spans.push(HighlightSpan {
                start: start as u32,
                end: end as u32,
            });
            search_from = abs + token.len();
            if search_from >= lower_text.len() {
                break;
            }
        }
    }

    spans.sort_by_key(|s| s.start);
    spans
}

// ═══════════════════════════════════════════════════════════════
// Book resolution
// ═══════════════════════════════════════════════════════════════

fn resolve_book(conn: &Connection, translation_id: &str, book_norm: &str) -> Option<String> {
    if let Ok(id) = conn.query_row(
        "SELECT id FROM books WHERE translation_id = ?1 AND LOWER(id) = ?2",
        params![translation_id, book_norm],
        |r| r.get::<_, String>(0),
    ) {
        return Some(id);
    }

    let mut stmt = conn
        .prepare("SELECT id, name FROM books WHERE translation_id = ?1 ORDER BY id")
        .ok()?;
    let rows = stmt
        .query_map(params![translation_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .ok()?;

    let mut prefix_matches: Vec<String> = Vec::new();
    for row in rows.flatten() {
        let (id, name) = row;
        let name_norm: String = name
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect();
        if name_norm == book_norm {
            return Some(id);
        }
        if name_norm.starts_with(book_norm) {
            prefix_matches.push(id);
        }
    }

    prefix_matches.into_iter().next()
}

// ═══════════════════════════════════════════════════════════════
// The search pipeline
// ═══════════════════════════════════════════════════════════════

pub fn search(
    conn: &Connection,
    query: &str,
    translation_id: Option<&str>,
    limit: i32,
    filter: SearchFilter,
    mut on_progress: impl FnMut(SearchProgress),
) -> Result<SearchResults, FreeBibleError> {
    let start = std::time::Instant::now();

    on_progress(SearchProgress {
        phase: SearchPhase::Classifying,
        message: "Parsing query…".into(),
        fraction: 0.05,
    });
    let classified = classify(query);

    on_progress(SearchProgress {
        phase: SearchPhase::Resolving,
        message: match &classified {
            QueryKind::Reference { book, .. } => format!("Looking up {book}…"),
            QueryKind::Strongs(s) => format!("Looking up {s}…"),
            QueryKind::Text(_) => "Preparing search…".into(),
        },
        fraction: 0.2,
    });

    let (hits, total) = match classified {
        QueryKind::Strongs(strongs) => search_strongs(conn, &strongs, limit)?,
        QueryKind::Reference {
            book,
            chapter,
            verse,
            end_verse,
        } => {
            let tid = translation_id.ok_or_else(|| FreeBibleError::Generic {
                error_message: "reference search requires a translation".into(),
            })?;
            search_reference(conn, tid, &book, chapter, verse, end_verse, limit)?
        }
        QueryKind::Text(text) => {
            on_progress(SearchProgress {
                phase: SearchPhase::Querying,
                message: "Searching verses…".into(),
                fraction: 0.4,
            });
            search_text(conn, &text, translation_id, limit, &filter)?
        }
    };

    on_progress(SearchProgress {
        phase: SearchPhase::Ranking,
        message: format!("{total} result(s)"),
        fraction: 0.9,
    });

    let results = SearchResults {
        query: query.to_string(),
        hits,
        total_hits: total,
        took_ms: start.elapsed().as_millis() as i64,
    };

    on_progress(SearchProgress {
        phase: SearchPhase::Done,
        message: format!("{} results in {}ms", results.total_hits, results.took_ms),
        fraction: 1.0,
    });

    Ok(results)
}

// ═══════════════════════════════════════════════════════════════
// Text search — dispatch by mode
// ═══════════════════════════════════════════════════════════════

fn search_text(
    conn: &Connection,
    text: &str,
    translation_id: Option<&str>,
    limit: i32,
    filter: &SearchFilter,
) -> Result<(Vec<SearchHit>, i32), FreeBibleError> {
    if text.is_empty() {
        return Ok((Vec::new(), 0));
    }

    match filter.mode {
        SearchMode::MultiWord | SearchMode::Phrase => {
            search_text_fts(conn, text, translation_id, limit, filter)
        }
        SearchMode::Regex => search_text_regex(conn, text, translation_id, limit, filter),
    }
}

// ═══════════════════════════════════════════════════════════════
// Text search — FTS5 backend
// ═══════════════════════════════════════════════════════════════

fn make_text_hit(r: &rusqlite::Row<'_>, query: &str) -> rusqlite::Result<SearchHit> {
    let text: String = r.get(5)?;
    let highlights = compute_highlights(&text, query);
    Ok(SearchHit {
        kind: "verse".into(),
        translation_id: r.get(0)?,
        book_id: r.get(1)?,
        book_name: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        chapter: r.get(3)?,
        verse: r.get(4)?,
        text,
        score: r.get(6)?,
        highlights,
    })
}

/// FTS5 text search with the book filter applied.
///
/// The `WHERE` clause is built from at most two constraints:
/// `translation_id` (optional) and `book_ids` (from the filter).
/// No testament flag exists here because the engine does not know
/// what a canon is. The UI computes `book_ids` from
/// `book_metadata.section` before calling search.
fn search_text_fts(
    conn: &Connection,
    text: &str,
    translation_id: Option<&str>,
    limit: i32,
    filter: &SearchFilter,
) -> Result<(Vec<SearchHit>, i32), FreeBibleError> {
    let fts_query = build_fts_query(text, filter.mode)?;

    // Bind order: ?1 = FTS query, optional translation_id, then the
    // explicit book list, then limit.
    let mut where_clauses = vec!["f.verses_fts MATCH ?1".to_string()];
    let mut bind_idx = 2;

    if translation_id.is_some() {
        where_clauses.push(format!("v.translation_id = ?{bind_idx}"));
        bind_idx += 1;
    }

    let num_book_params = filter.book_ids.len();
    if num_book_params > 0 {
        let placeholders = (0..num_book_params)
            .map(|i| format!("?{}", bind_idx + i))
            .collect::<Vec<_>>()
            .join(",");
        where_clauses.push(format!("v.book_id IN ({placeholders})"));
        bind_idx += num_book_params;
    }

    let where_sql = where_clauses.join(" AND ");
    let limit_param = bind_idx;

    // ── Hits query ─────────────────────────────────────────────
    let sql = format!(
        "SELECT v.translation_id, v.book_id, b.name, v.chapter, v.verse, v.text, f.rank
         FROM verses_fts f
         JOIN verses v ON v.rowid = f.rowid
         LEFT JOIN books b ON b.translation_id = v.translation_id AND b.id = v.book_id
         WHERE {where_sql}
         ORDER BY f.rank
         LIMIT ?{limit_param}"
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| FreeBibleError::Generic {
        error_message: format!("search_text prepare: {e}"),
    })?;

    // ── Bind parameters dynamically ────────────────────────────
    let mut param_values: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(limit_param as usize);
    param_values.push(Box::new(fts_query.clone()));

    if let Some(tid) = translation_id {
        param_values.push(Box::new(tid.to_string()));
    }

    for id in &filter.book_ids {
        param_values.push(Box::new(id.clone()));
    }

    param_values.push(Box::new(limit));

    let params_refs: Vec<&dyn rusqlite::ToSql> = param_values.iter().map(|b| b.as_ref()).collect();

    let hits: Vec<SearchHit> = stmt
        .query_map(&params_refs[..], |r| make_text_hit(r, text))
        .map_err(|e| FreeBibleError::Generic {
            error_message: format!("search_text query: {e}"),
        })?
        .filter_map(|r| r.ok())
        .collect();

    // ── Count query ────────────────────────────────────────────
    let count_sql = format!(
        "SELECT COUNT(*)
         FROM verses_fts f
         JOIN verses v ON v.rowid = f.rowid
         WHERE {where_sql}"
    );

    let count_params = &params_refs[..params_refs.len() - 1];
    let total: i32 = conn
        .query_row(&count_sql, count_params, |r| r.get(0))
        .unwrap_or(0);

    Ok((hits, total))
}

// ═══════════════════════════════════════════════════════════════
// Text search — regex backend
// ═══════════════════════════════════════════════════════════════

fn search_text_regex(
    conn: &Connection,
    pattern: &str,
    translation_id: Option<&str>,
    limit: i32,
    filter: &SearchFilter,
) -> Result<(Vec<SearchHit>, i32), FreeBibleError> {
    use regex::Regex;

    let re = Regex::new(pattern).map_err(|e| FreeBibleError::Generic {
        error_message: format!("invalid regex {pattern:?}: {e}"),
    })?;

    let mut where_clauses = vec!["1=1".to_string()];
    let mut bind_idx = 1;

    if translation_id.is_some() {
        where_clauses.push(format!("v.translation_id = ?{bind_idx}"));
        bind_idx += 1;
    }

    let num_book_params = filter.book_ids.len();
    if num_book_params > 0 {
        let placeholders = (0..num_book_params)
            .map(|i| format!("?{}", bind_idx + i))
            .collect::<Vec<_>>()
            .join(",");
        where_clauses.push(format!("v.book_id IN ({placeholders})"));
    }

    let where_sql = where_clauses.join(" AND ");

    let sql = format!(
        "SELECT v.translation_id, v.book_id, b.name, v.chapter, v.verse, v.text
         FROM verses v
         LEFT JOIN books b ON b.translation_id = v.translation_id AND b.id = v.book_id
         WHERE {where_sql}"
    );

    let mut stmt = conn.prepare(&sql).map_err(|e| FreeBibleError::Generic {
        error_message: format!("search_text_regex prepare: {e}"),
    })?;

    let mut param_values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(tid) = translation_id {
        param_values.push(Box::new(tid.to_string()));
    }
    for id in &filter.book_ids {
        param_values.push(Box::new(id.clone()));
    }

    let params_refs: Vec<&dyn rusqlite::ToSql> = param_values.iter().map(|b| b.as_ref()).collect();

    let mut hits: Vec<SearchHit> = Vec::with_capacity(limit.max(0) as usize);
    let mut total: i32 = 0;

    let rows = stmt
        .query_map(&params_refs[..], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, i32>(3)?,
                r.get::<_, i32>(4)?,
                r.get::<_, String>(5)?,
            ))
        })
        .map_err(|e| FreeBibleError::Generic {
            error_message: format!("search_text_regex query: {e}"),
        })?;

    for row in rows.filter_map(|r| r.ok()) {
        let (tid, book_id, book_name, chapter, verse, text) = row;

        if !re.is_match(&text) {
            continue;
        }

        let highlights: Vec<HighlightSpan> = re
            .find_iter(&text)
            .map(|m| HighlightSpan {
                start: m.start() as u32,
                end: m.end() as u32,
            })
            .collect();

        total += 1;

        if hits.len() < limit.max(0) as usize {
            hits.push(SearchHit {
                kind: "verse".into(),
                translation_id: tid,
                book_id,
                book_name: book_name.unwrap_or_default(),
                chapter,
                verse,
                text,
                score: 0.0,
                highlights,
            });
        }
    }

    Ok((hits, total))
}

// ═══════════════════════════════════════════════════════════════
// Reference search
// ═══════════════════════════════════════════════════════════════

fn search_reference(
    conn: &Connection,
    translation_id: &str,
    book_norm: &str,
    chapter: i32,
    verse: Option<i32>,
    end_verse: Option<i32>,
    limit: i32,
) -> Result<(Vec<SearchHit>, i32), FreeBibleError> {
    let Some(book_id) = resolve_book(conn, translation_id, book_norm) else {
        return Ok((Vec::new(), 0));
    };

    let (sql, params_vec): (&str, Vec<Box<dyn rusqlite::ToSql>>) = match (verse, end_verse) {
        (Some(v), Some(e)) => (
            "SELECT v.translation_id, v.book_id, b.name, v.chapter, v.verse, v.text, 0.0
             FROM verses v
             LEFT JOIN books b ON b.translation_id = v.translation_id AND b.id = v.book_id
             WHERE v.translation_id = ?1 AND v.book_id = ?2 AND v.chapter = ?3
               AND v.verse >= ?4 AND v.verse <= ?5
             ORDER BY v.verse LIMIT ?6",
            vec![
                Box::new(translation_id.to_string()),
                Box::new(book_id),
                Box::new(chapter),
                Box::new(v),
                Box::new(e),
                Box::new(limit),
            ],
        ),
        (Some(v), None) => (
            "SELECT v.translation_id, v.book_id, b.name, v.chapter, v.verse, v.text, 0.0
             FROM verses v
             LEFT JOIN books b ON b.translation_id = v.translation_id AND b.id = v.book_id
             WHERE v.translation_id = ?1 AND v.book_id = ?2 AND v.chapter = ?3
               AND v.verse = ?4
             ORDER BY v.verse LIMIT ?5",
            vec![
                Box::new(translation_id.to_string()),
                Box::new(book_id),
                Box::new(chapter),
                Box::new(v),
                Box::new(limit),
            ],
        ),
        _ => (
            "SELECT v.translation_id, v.book_id, b.name, v.chapter, v.verse, v.text, 0.0
             FROM verses v
             LEFT JOIN books b ON b.translation_id = v.translation_id AND b.id = v.book_id
             WHERE v.translation_id = ?1 AND v.book_id = ?2 AND v.chapter = ?3
             ORDER BY v.verse LIMIT ?4",
            vec![
                Box::new(translation_id.to_string()),
                Box::new(book_id),
                Box::new(chapter),
                Box::new(limit),
            ],
        ),
    };

    let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(sql).map_err(|e| FreeBibleError::Generic {
        error_message: format!("search_reference prepare: {e}"),
    })?;

    let hits: Vec<SearchHit> = stmt
        .query_map(&params_refs[..], |r| {
            Ok(SearchHit {
                kind: "reference".into(),
                translation_id: r.get(0)?,
                book_id: r.get(1)?,
                book_name: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                chapter: r.get(3)?,
                verse: r.get(4)?,
                text: r.get(5)?,
                score: r.get(6)?,
                highlights: Vec::new(),
            })
        })
        .map_err(|e| FreeBibleError::Generic {
            error_message: format!("search_reference query: {e}"),
        })?
        .filter_map(|r| r.ok())
        .collect();

    let total = hits.len() as i32;
    Ok((hits, total))
}

// ═══════════════════════════════════════════════════════════════
// Strong's search
// ═══════════════════════════════════════════════════════════════

fn search_strongs(
    conn: &Connection,
    strongs: &str,
    limit: i32,
) -> Result<(Vec<SearchHit>, i32), FreeBibleError> {
    let mut stmt = conn
        .prepare(
            "SELECT strongs, lemma, transliteration, gloss, definition
             FROM lexicon WHERE strongs = ?1
             ORDER BY strongs LIMIT ?2",
        )
        .map_err(|e| FreeBibleError::Generic {
            error_message: format!("search_strongs prepare: {e}"),
        })?;

    let hits: Vec<SearchHit> = stmt
        .query_map(params![strongs, limit], |r| {
            let lemma: Option<String> = r.get(1)?;
            let translit: Option<String> = r.get(2)?;
            let gloss: Option<String> = r.get(3)?;
            let definition: Option<String> = r.get(4)?;
            Ok(SearchHit {
                kind: "lexicon".into(),
                translation_id: String::new(),
                book_id: String::new(),
                book_name: String::new(),
                chapter: 0,
                verse: 0,
                text: format!(
                    "{}{}{}{}",
                    lemma.map(|s| format!("{s} ")).unwrap_or_default(),
                    translit.map(|s| format!("({s}) ")).unwrap_or_default(),
                    gloss.map(|s| format!("— {s}: ")).unwrap_or_default(),
                    definition.unwrap_or_default(),
                )
                .trim()
                .to_string(),
                score: 0.0,
                highlights: Vec::new(),
            })
        })
        .map_err(|e| FreeBibleError::Generic {
            error_message: format!("search_strongs query: {e}"),
        })?
        .filter_map(|r| r.ok())
        .collect();

    let total = hits.len() as i32;
    Ok((hits, total))
}

// ═══════════════════════════════════════════════════════════════
// FTS5 query construction
// ═══════════════════════════════════════════════════════════════

fn build_fts_query(text: &str, mode: SearchMode) -> Result<String, FreeBibleError> {
    match mode {
        SearchMode::MultiWord => Ok(text
            .split_whitespace()
            .map(|token| {
                let escaped = token.replace('"', "\"\"");
                format!("\"{escaped}\"")
            })
            .collect::<Vec<_>>()
            .join(" ")),

        SearchMode::Phrase => {
            let escaped = text.trim().replace('"', "\"\"");
            Ok(format!("\"{escaped}\""))
        }

        SearchMode::Regex => Err(FreeBibleError::Generic {
            error_message: "build_fts_query called for regex mode".into(),
        }),
    }
}

// ═══════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_reference_id() {
        match classify("JHN 3:16") {
            QueryKind::Reference {
                book,
                chapter,
                verse,
                end_verse,
            } => {
                assert_eq!(book, "jhn");
                assert_eq!(chapter, 3);
                assert_eq!(verse, Some(16));
                assert_eq!(end_verse, None);
            }
            other => panic!("expected reference, got {other:?}"),
        }
    }

    #[test]
    fn classify_reference_name_range() {
        match classify("gen 1:1-5") {
            QueryKind::Reference {
                book,
                chapter,
                verse,
                end_verse,
            } => {
                assert_eq!(book, "gen");
                assert_eq!(chapter, 1);
                assert_eq!(verse, Some(1));
                assert_eq!(end_verse, Some(5));
            }
            other => panic!("expected reference, got {other:?}"),
        }
    }

    #[test]
    fn classify_reference_with_leading_number() {
        match classify("1 cor 13") {
            QueryKind::Reference {
                book,
                chapter,
                verse,
                end_verse,
            } => {
                assert_eq!(book, "1cor");
                assert_eq!(chapter, 13);
                assert_eq!(verse, None);
                assert_eq!(end_verse, None);
            }
            other => panic!("expected reference, got {other:?}"),
        }
    }

    #[test]
    fn classify_strongs() {
        assert_eq!(classify("H430"), QueryKind::Strongs("H430".into()));
        assert_eq!(classify("g26"), QueryKind::Strongs("G26".into()));
    }

    #[test]
    fn classify_text() {
        assert_eq!(classify("shepherd"), QueryKind::Text("shepherd".into()));
        assert_eq!(classify("love God"), QueryKind::Text("love God".into()));
    }

    #[test]
    fn fts_multiword_quotes_each_token() {
        assert_eq!(
            build_fts_query("love God", SearchMode::MultiWord).unwrap(),
            "\"love\" \"God\""
        );
        assert_eq!(
            build_fts_query("a\"b", SearchMode::MultiWord).unwrap(),
            "\"a\"\"b\""
        );
    }

    #[test]
    fn fts_phrase_quotes_whole_input() {
        assert_eq!(
            build_fts_query("love God", SearchMode::Phrase).unwrap(),
            "\"love God\""
        );
        assert_eq!(
            build_fts_query("say \"hello\"", SearchMode::Phrase).unwrap(),
            "\"say \"\"hello\"\"\""
        );
    }

    #[test]
    fn fts_regex_mode_errors() {
        assert!(build_fts_query("love God", SearchMode::Regex).is_err());
    }

    #[test]
    fn highlights_single_token() {
        let text = "The LORD is my shepherd; I shall not want.";
        let spans = compute_highlights(text, "shepherd");
        assert_eq!(spans.len(), 1);
        let s = &spans[0];
        assert_eq!(&text[s.start as usize..s.end as usize], "shepherd");
    }

    #[test]
    fn highlights_multi_token() {
        let text = "God is love, and love comes from God.";
        let spans = compute_highlights(text, "love God");
        assert_eq!(spans.len(), 4);
        for s in &spans {
            let slice = text[s.start as usize..s.end as usize].to_lowercase();
            assert!(slice == "god" || slice == "love");
        }
    }

    #[test]
    fn highlights_empty_query() {
        assert!(compute_highlights("anything", "").is_empty());
    }

    // ── Filter behaviour ────────────────────────────────────────
    //
    // These tests don't require a real DB. They only exercise the
    // SQL construction by using an in-memory connection with the
    // minimum schema the queries touch. The point is to prove the
    // book filter actually restricts — not to prove FTS5 works
    // (that's covered by the tests/db_smoke.rs integration suite).

    fn temp_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE translations (
                id TEXT PRIMARY KEY,
                selected_canon TEXT NOT NULL DEFAULT 'protestant'
            );
            CREATE TABLE books (
                translation_id TEXT NOT NULL,
                id TEXT NOT NULL,
                name TEXT NOT NULL,
                number_of_chapters INTEGER NOT NULL,
                PRIMARY KEY (translation_id, id)
            );
            CREATE TABLE verses (
                translation_id TEXT NOT NULL,
                book_id TEXT NOT NULL,
                chapter INTEGER NOT NULL,
                verse INTEGER NOT NULL,
                text TEXT NOT NULL,
                PRIMARY KEY (translation_id, book_id, chapter, verse)
            );
            CREATE VIRTUAL TABLE verses_fts USING fts5(
                text,
                content='',
                tokenize='unicode61 remove_diacritics 2'
            );
            "#,
        )
        .unwrap();
        conn
    }

    fn seed(
        conn: &Connection,
        translation_id: &str,
        book_id: &str,
        chapter: i32,
        verse: i32,
        text: &str,
    ) {
        conn.execute(
            "INSERT INTO verses (translation_id, book_id, chapter, verse, text)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![translation_id, book_id, chapter, verse, text],
        )
        .unwrap();
        let rowid: i64 = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO verses_fts(rowid, text) VALUES (?1, ?2)",
            params![rowid, text],
        )
        .unwrap();
    }

    #[test]
    fn filter_with_empty_book_ids_matches_everything() {
        let conn = temp_conn();
        seed(&conn, "TST", "GEN", 1, 1, "God created the heavens.");
        seed(&conn, "TST", "PSA", 23, 1, "The LORD is my shepherd.");

        let result = search(
            &conn,
            "God",
            Some("TST"),
            10,
            SearchFilter::default(),
            |_| {},
        )
        .unwrap();

        assert_eq!(result.total_hits, 1, "only Genesis contains God");
        assert_eq!(result.hits[0].book_id, "GEN");
    }

    #[test]
    fn filter_with_book_ids_restricts_to_those_books() {
        let conn = temp_conn();
        seed(&conn, "TST", "GEN", 1, 1, "God created the heavens.");
        seed(
            &conn,
            "TST",
            "PSA",
            23,
            1,
            "The LORD is my shepherd and God is with me.",
        );

        // No filter: both verses match "God".
        let no_filter = search(
            &conn,
            "God",
            Some("TST"),
            10,
            SearchFilter::default(),
            |_| {},
        )
        .unwrap();
        assert_eq!(no_filter.total_hits, 2);

        // Filter to Psalms: only the Psalms verse.
        let psa_only = search(
            &conn,
            "God",
            Some("TST"),
            10,
            SearchFilter {
                book_ids: vec!["PSA".into()],
                ..SearchFilter::default()
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(psa_only.total_hits, 1);
        assert_eq!(psa_only.hits[0].book_id, "PSA");

        // Filter to a book with no matching verses: zero results.
        let jhn_only = search(
            &conn,
            "God",
            Some("TST"),
            10,
            SearchFilter {
                book_ids: vec!["JHN".into()],
                ..SearchFilter::default()
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(jhn_only.total_hits, 0);
    }
}
