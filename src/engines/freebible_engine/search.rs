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
//! ## Why FTS5 for text and not `LIKE`
//!
//! `LIKE '%shepherd%'` cannot use an index; it scans every verse row.
//! For one translation that is ~31,000 rows and ~50 ms. For five it is
//! ~150,000 rows and ~500 ms on Android mid-range hardware. FTS5 keeps
//! an inverted index and answers the same query in ~1 ms, and returns
//! BM25-ranked results for free. The FTS table is populated by
//! triggers on `verses`, so the install path needed no changes.
//!
//! ## Why regex bypasses FTS5
//!
//! FTS5 has no regex support. A regex is an arbitrary pattern over the
//! text, which the inverted index cannot answer. Regex search scans
//! every verse's text with a compiled regex — ~200 ms for one
//! translation. It is slower than FTS5 but it is the only way to
//! support arbitrary patterns. The same `SearchFilter` constrains it
//! to one book or testament, so "regex over Psalms" scans 2,461 rows,
//! not 31,086.
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
//! ## Filters
//!
//! [`SearchFilter`] constrains a search to one translation, a list of
//! books, or a testament. All three combine with `AND`. An explicit
//! `book_ids` list wins over the testament flags — a user who picks
//! "Psalms" and "Old Testament" means "Psalms", not "all OT books".
//!
//! ## Progress reporting
//!
//! [`search`] emits [`SearchProgress`] events between phases:
//! Classifying → Resolving → Querying → Ranking → Done. For a 1 ms
//! search these fire faster than the UI can render, so they are
//! primarily useful for the app to log a slow search or to show a
//! spinner while the FTS index is being paged in on first use.
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
///
/// `start` is inclusive, `end` is exclusive. Both are UTF-8 byte
/// offsets into the string, which for ASCII and Latin-1 text equals
/// the character offset. The UI slices `hit.text[start..end]` to
/// extract the matched substring.
#[derive(Debug, Clone, uniffi::Record)]
pub struct HighlightSpan {
    /// Byte offset where the match starts.
    pub start: u32,
    /// Byte offset where the match ends (exclusive).
    pub end: u32,
}

/// How a text query is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, uniffi::Enum)]
pub enum SearchMode {
    /// All words must appear, any order. `love God` matches
    /// "God is love".
    #[default]
    MultiWord,

    /// The words must appear as an exact adjacent sequence.
    /// `love God` matches only "love God".
    Phrase,

    /// Regular expression over the verse text. Slower than FTS5;
    /// bypasses the inverted index entirely.
    Regex,
}

/// Constraints on a search.
///
/// All fields are optional. `SearchFilter::default()` means "search
/// everything, multi-word mode", which is the correct behavior for a
/// frontend that does not expose filters.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct SearchFilter {
    /// Only search within these book IDs (`"PSA"`, `"JHN"`, …). Empty
    /// means all books. An explicit list overrides the testament flags
    /// below.
    pub book_ids: Vec<String>,

    /// Search only the Old Testament (39 books).
    pub old_testament_only: bool,

    /// Search only the New Testament (27 books).
    pub new_testament_only: bool,

    /// How to interpret the text query. Applies only to text search;
    /// references and Strong's lookups ignore it.
    pub mode: SearchMode,
}

/// One step of the search pipeline. Emitted via [`SearchProgress`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SearchPhase {
    /// Parsing the query into Reference / Strongs / Text.
    Classifying,
    /// Looking up the book or lexicon entry in the DB.
    Resolving,
    /// Running the FTS5 or regex query.
    Querying,
    /// Sorting and counting the results.
    Ranking,
    /// Returning the final [`SearchResults`].
    Done,
}

/// A progress event emitted by [`search`].
#[derive(Debug, Clone, uniffi::Record)]
pub struct SearchProgress {
    pub phase: SearchPhase,
    pub message: String,
    /// 0.0 → 1.0, best-effort. Phases that do no measurable work
    /// emit a fixed fraction rather than a fake percentage.
    pub fraction: f64,
}

/// One result of a search.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SearchHit {
    /// `"verse"` | `"reference"` | `"lexicon"`
    pub kind: String,
    pub translation_id: String,
    pub book_id: String,
    pub book_name: String,
    pub chapter: i32,
    pub verse: i32,
    pub text: String,
    /// Lower is better for BM25. Ignored for reference and lexicon
    /// hits, which always return `0.0`.
    pub score: f64,
    /// Byte ranges in `text` that matched the query. Empty for
    /// reference and lexicon hits — only text search highlights.
    pub highlights: Vec<HighlightSpan>,
}

/// The complete result of a search, including timing.
#[derive(Debug, Clone, uniffi::Record)]
pub struct SearchResults {
    pub query: String,
    pub hits: Vec<SearchHit>,
    /// Total hits across the whole result set, not just `hits.len()`.
    /// For a search with `limit = 20` and 62 total matches,
    /// `hits.len() == 20` and `total_hits == 62`.
    pub total_hits: i32,
    pub took_ms: i64,
}

// ═══════════════════════════════════════════════════════════════
// Query classification
// ═══════════════════════════════════════════════════════════════

/// What kind of query the user typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryKind {
    /// `"JHN 3:16"`, `"john 3"`, `"gen 1:1-5"`, `"1 cor 13"`
    Reference {
        book: String,
        chapter: i32,
        verse: Option<i32>,
        end_verse: Option<i32>,
    },
    /// `"H430"`, `"G26"`
    Strongs(String),
    /// `"shepherd"`, `"love of God"`, `"ACAKI"`
    Text(String),
}

/// Parse a user query into one of the three shapes.
///
/// Deliberately simple: it recognizes the forms users actually type
/// for Bible search. It does not try to be a general NLP parser, and
/// it never falls back to a "did you mean" guess — an unparseable
/// reference is treated as text search, which is what the user
/// expects when they typed a word.
pub fn classify(query: &str) -> QueryKind {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return QueryKind::Text(String::new());
    }

    // Strong's: H or G followed by 1-4 digits, whole token.
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

/// Parse forms like:
///   `"JHN 3:16"`, `"john 3"`, `"gen 1:1-5"`, `"1 cor 13"`, `"1cor13"`
///
/// The book part is the leading alphabetic tokens (with optional
/// leading number); the rest is `chapter[:verse[-endverse]]`. Returns
/// `None` if the shape does not match — the caller then treats the
/// query as text search.
fn parse_reference(input: &str) -> Option<ParsedRef> {
    // Split at the first digit that comes *after* at least one
    // alphabetic char. That handles "1 cor 13" (book name starts
    // with a digit) without splitting at the "1".
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

    // Normalize the book name: lowercase, strip spaces and dots.
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

/// Find every occurrence of `query`'s tokens in `text`,
/// case-insensitively, and return byte spans.
///
/// Multi-word queries return spans for each token separately, sorted
/// by position. Phrase queries return the single span for the whole
/// phrase. Regex queries should not call this — the UI computes
/// highlights from the regex match positions directly.
///
/// Tokenization mirrors [`build_fts_query`]: split on whitespace,
/// strip surrounding quotes, lowercase. This is not a full Unicode
/// word-boundary search, but it matches what FTS5 indexed, which is
/// what the user sees highlighted.
///
/// **Byte-offset caveat:** `text.to_lowercase()` can change byte
/// length (`ß` → `ss`, `İ` → `i̇`). For ASCII and Latin-1 text — which
/// covers BSB, KJV, and ach_abr — lowercasing is
/// byte-length-preserving and the offsets are exact. For text with
/// combining marks they may be off by a byte or two, which is
/// acceptable for highlighting. A fully Unicode-correct version
/// needs char-index mapping, which is a v8 concern.
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

/// Resolve a normalized book name (lowercase alphanumeric) against
/// the `books` table. Matches, in priority order:
///
/// 1. exact `id` (case-insensitive) — `"jhn"`
/// 2. exact `name` (case-insensitive, alnum-only) — `"john"`
/// 3. `name` prefix — `"joh"` matches "John"
///
/// Ambiguity (e.g. `"jo"` matching "Job" and "John") is resolved by
/// the first match in `id` order, which for a standard translation
/// is canonical book order.
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

/// Execute a search and return the results.
///
/// This is the only public entry point. It classifies the query,
/// dispatches to the appropriate backend, and returns a
/// [`SearchResults`] with hits, total count, and timing.
///
/// `on_progress` is called between phases. For a 1 ms search the
/// events fire faster than the UI can render; the primary use is
/// logging a slow search or showing a spinner while the FTS index
/// pages in on first use.
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

/// Dispatch a text query to the FTS5 backend (multi-word, phrase)
/// or the regex backend. The two paths share the same
/// [`SearchFilter`] but use entirely different SQL.
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
// Text search — FTS5 backend (multi-word, phrase)
// ═══════════════════════════════════════════════════════════════

/// Build one `SearchHit` from an FTS5 result row.
///
/// The `query` is the raw user text, used to compute highlight
/// spans against the joined `verses.text`. The FTS5 rank is the
/// BM25 score; lower is better.
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

/// FTS5 text search with dynamic filters.
///
/// The `WHERE` clause is built from three optional constraints:
/// `translation_id`, an explicit `book_ids` list, and the
/// testament flags. Each constraint reserves a contiguous range of
/// `?N` placeholders, so the final SQL has the right number of
/// parameters without the caller having to count.
fn search_text_fts(
    conn: &Connection,
    text: &str,
    translation_id: Option<&str>,
    limit: i32,
    filter: &SearchFilter,
) -> Result<(Vec<SearchHit>, i32), FreeBibleError> {
    let fts_query = build_fts_query(text, filter.mode)?;

    // Canonical book ID lists. Kept here so the frontend does not
    // have to know which books belong to which testament. Every
    // installed translation uses these same USFM-style IDs.
    const OT_IDS: &[&str] = &[
        "GEN", "EXO", "LEV", "NUM", "DEU", "JOS", "JDG", "RUT", "1SA", "2SA", "1KI", "2KI", "1CH",
        "2CH", "EZR", "NEH", "EST", "JOB", "PSA", "PRO", "ECC", "SNG", "ISA", "JER", "LAM", "EZK",
        "DAN", "HOS", "JOL", "AMO", "OBA", "JON", "MIC", "NAM", "HAB", "ZEP", "HAG", "ZEC", "MAL",
    ];
    const NT_IDS: &[&str] = &[
        "MAT", "MRK", "LUK", "JHN", "ACT", "ROM", "1CO", "2CO", "GAL", "EPH", "PHP", "COL", "1TH",
        "2TH", "1TI", "2TI", "TIT", "PHM", "HEB", "JAS", "1PE", "2PE", "1JN", "2JN", "3JN", "JUD",
        "REV",
    ];

    // ── Build the dynamic WHERE clause ──────────────────────────
    //
    // Bind order matters: ?1 is always the FTS query, then optional
    // translation_id, then the book list, then limit.
    let mut where_clauses = vec!["f.verses_fts MATCH ?1".to_string()];
    let mut bind_idx = 2;

    if translation_id.is_some() {
        where_clauses.push(format!("v.translation_id = ?{bind_idx}"));
        bind_idx += 1;
    }

    // Which book IDs to constrain to, if any. An explicit list wins
    // over the testament flags: a user who picks "Psalms" and "Old
    // Testament" means "Psalms", not "all OT books".
    let explicit_books: Option<&[String]> = if !filter.book_ids.is_empty() {
        Some(&filter.book_ids)
    } else {
        None
    };

    let testament_ids: Option<&[&str]> = if explicit_books.is_some() {
        None
    } else if filter.old_testament_only {
        Some(OT_IDS)
    } else if filter.new_testament_only {
        Some(NT_IDS)
    } else {
        None
    };

    let num_book_params = if let Some(books) = explicit_books {
        books.len()
    } else if let Some(ids) = testament_ids {
        ids.len()
    } else {
        0
    };

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
    //
    // Order must match the ?1..?N placeholders in the SQL above:
    //   1. fts_query
    //   2. translation_id (if present)
    //   3..N+2. book list
    //   N+3. limit
    let mut param_values: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(limit_param as usize);
    param_values.push(Box::new(fts_query.clone()));

    if let Some(tid) = translation_id {
        param_values.push(Box::new(tid.to_string()));
    }

    if let Some(books) = explicit_books {
        for id in books {
            param_values.push(Box::new(id.clone()));
        }
    } else if let Some(ids) = testament_ids {
        for id in ids {
            param_values.push(Box::new((*id).to_string()));
        }
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
    //
    // Same WHERE clause, no LIMIT. Reuse the same parameter slice
    // except drop the trailing `limit` value.
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

/// Regex search over `verses.text`.
///
/// This does not use FTS5 at all — it scans every verse that passes
/// the book/translation filter. For a filter that constrains to one
/// book, the scan is a few thousand rows; for an unconstrained
/// search it is the whole translation. Either way it is slower than
/// FTS5 (200 ms vs 1 ms for BSB) but it is the only way to support
/// arbitrary patterns.
///
/// Highlights are not computed here. The UI has the same regex, so
/// it can compute match positions itself without another round-trip.
/// The cost is a one-line `regex.findAll()` in the frontend vs a
/// byte-offset conversion in Rust that is easy to get wrong.
fn search_text_regex(
    conn: &Connection,
    pattern: &str,
    translation_id: Option<&str>,
    limit: i32,
    filter: &SearchFilter,
) -> Result<(Vec<SearchHit>, i32), FreeBibleError> {
    use regex::Regex;

    // Compile once. A bad pattern returns a clear error, not a
    // silent zero-hit result.
    let re = Regex::new(pattern).map_err(|e| FreeBibleError::Generic {
        error_message: format!("invalid regex {pattern:?}: {e}"),
    })?;

    const OT_IDS: &[&str] = &[
        "GEN", "EXO", "LEV", "NUM", "DEU", "JOS", "JDG", "RUT", "1SA", "2SA", "1KI", "2KI", "1CH",
        "2CH", "EZR", "NEH", "EST", "JOB", "PSA", "PRO", "ECC", "SNG", "ISA", "JER", "LAM", "EZK",
        "DAN", "HOS", "JOL", "AMO", "OBA", "JON", "MIC", "NAM", "HAB", "ZEP", "HAG", "ZEC", "MAL",
    ];
    const NT_IDS: &[&str] = &[
        "MAT", "MRK", "LUK", "JHN", "ACT", "ROM", "1CO", "2CO", "GAL", "EPH", "PHP", "COL", "1TH",
        "2TH", "1TI", "2TI", "TIT", "PHM", "HEB", "JAS", "1PE", "2PE", "1JN", "2JN", "3JN", "JUD",
        "REV",
    ];

    // ── Build the dynamic WHERE clause ──────────────────────────
    let mut where_clauses = vec!["1=1".to_string()];
    let mut bind_idx = 1;

    if translation_id.is_some() {
        where_clauses.push(format!("v.translation_id = ?{bind_idx}"));
        bind_idx += 1;
    }

    let explicit_books: Option<&[String]> = if !filter.book_ids.is_empty() {
        Some(&filter.book_ids)
    } else {
        None
    };
    let testament_ids: Option<&[&str]> = if explicit_books.is_some() {
        None
    } else if filter.old_testament_only {
        Some(OT_IDS)
    } else if filter.new_testament_only {
        Some(NT_IDS)
    } else {
        None
    };

    let num_book_params = if let Some(books) = explicit_books {
        books.len()
    } else if let Some(ids) = testament_ids {
        ids.len()
    } else {
        0
    };

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

    // ── Bind params ───────────────────────────────────────────
    let mut param_values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    if let Some(tid) = translation_id {
        param_values.push(Box::new(tid.to_string()));
    }
    if let Some(books) = explicit_books {
        for id in books {
            param_values.push(Box::new(id.clone()));
        }
    } else if let Some(ids) = testament_ids {
        for id in ids {
            param_values.push(Box::new((*id).to_string()));
        }
    }

    let params_refs: Vec<&dyn rusqlite::ToSql> = param_values.iter().map(|b| b.as_ref()).collect();

    // ── Scan rows and filter by regex ────────────────────────
    //
    // No LIMIT in the SQL because the SQL does not know which rows
    // the regex will match. We scan every row the filter allows,
    // then take the first `limit` that match. For a translation
    // with 31,000 verses that is 31,000 regex tests at ~6 µs each,
    // which is ~200 ms — acceptable for a user-initiated search.
    let mut hits: Vec<SearchHit> = Vec::with_capacity(limit.max(0) as usize);
    let mut total: i32 = 0;

    let rows = stmt
        .query_map(&params_refs[..], |r| {
            Ok((
                r.get::<_, String>(0)?,         // translation_id
                r.get::<_, String>(1)?,         // book_id
                r.get::<_, Option<String>>(2)?, // book_name
                r.get::<_, i32>(3)?,            // chapter
                r.get::<_, i32>(4)?,            // verse
                r.get::<_, String>(5)?,         // text
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

        // Only build the SearchHit for the first `limit` matches.
        // Beyond that we still need to count so `total_hits` is
        // accurate, but we don't need the full struct.
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
                // Highlights are left empty — the UI computes them
                // from the same regex if it wants to render them.
                highlights: highlights,
            });
        }
    }

    Ok((hits, total))
}

// ═══════════════════════════════════════════════════════════════
// Reference search
// ═══════════════════════════════════════════════════════════════

/// Look up a Bible reference like `JHN 3:16` or `gen 1:1-5`.
///
/// Three shapes are possible:
///   - `chapter` only: `gen 1` → every verse in Genesis 1
///   - `chapter:verse`: `jhn 3:16` → exactly one verse
///   - `chapter:verse-endverse`: `gen 1:1-5` → five verses
///
/// The book part is resolved against `books` with three tiers
/// (exact id, exact name, name prefix). If the book is not found,
/// this returns an empty result rather than an error — a user who
/// types `zzz 1:1` should see "no results", not a crash.
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

/// Look up a Strong's number in the lexicon.
///
/// The lexicon is not per-translation — the same H430 entry applies
/// to every installed Bible — so the translation filter does not
/// apply here. Returns one hit per lexicon entry matching the id.
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

/// Build the FTS5 MATCH expression for `text` under `mode`.
///
/// - **MultiWord**: each whitespace-separated token is quoted and
///   ANDed. `love God` → `"love" "God"`. Matches any verse that
///   contains both tokens in any order.
///
/// - **Phrase**: the whole input is quoted as one phrase, with
///   inner quotes escaped. `love God` → `"love God"`. Matches only
///   the adjacent two-word sequence.
///
/// - **Regex**: returns an error, because regex does not go through
///   FTS5. `search_text` checks the mode and takes a different path
///   before it calls this.
///
/// Quoting every token (not just the multi-word case) is what
/// prevents users from injecting FTS5 operators. `AND`, `OR`,
/// `NEAR`, and `*` are all meaningful in raw FTS5 syntax; inside
/// quotes they are literals. A user who types `love OR God` gets
/// a search for the literal word `OR`, which is what they typed.
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

    // ── Classification ──────────────────────────────────────────

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

    // ── FTS query construction ──────────────────────────────────

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
        // Inner quotes escape as "" so the whole thing stays one
        // FTS5 phrase.
        assert_eq!(
            build_fts_query("say \"hello\"", SearchMode::Phrase).unwrap(),
            "\"say \"\"hello\"\"\""
        );
    }

    #[test]
    fn fts_regex_mode_errors() {
        assert!(build_fts_query("love God", SearchMode::Regex).is_err());
    }

    // ── Highlighting ────────────────────────────────────────────

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
}
