use rusqlite::{Connection, params};

use crate::engines::freebible_engine::error::FreeBibleError;

// ── Result types ──────────────────────────────────────────────

#[derive(Debug, Clone, uniffi::Record)]
pub struct HighlightSpan {
    /// Byte offset into `SearchHit.text` where the match starts.
    pub start: u32,
    /// Byte offset into `SearchHit.text` where the match ends (exclusive).
    pub end: u32,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct SearchFilter {
    /// Only search within these book IDs (e.g. ["PSA", "PRO"]). Empty means all books.
    pub book_ids: Vec<String>,
    /// Search only the Old Testament (39 books)
    pub old_testament_only: bool,
    /// Search only the New Testament (27 books)
    pub new_testament_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SearchPhase {
    Classifying,
    Resolving,
    Querying,
    Ranking,
    Done,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SearchProgress {
    pub phase: SearchPhase,
    pub message: String,
    /// 0.0 → 1.0, best-effort
    pub fraction: f64,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SearchHit {
    /// "verse" | "reference" | "lexicon"
    pub kind: String,
    pub translation_id: String,
    pub book_id: String,
    pub book_name: String,
    pub chapter: i32,
    pub verse: i32,
    pub text: String,
    /// Lower is better for BM25; ignored for references and lexicon.
    pub score: f64,
    /// Byte ranges in `text` that matched the query. Empty for
    /// reference and lexicon hits — only text search highlights.
    pub highlights: Vec<HighlightSpan>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct SearchResults {
    pub query: String,
    pub hits: Vec<SearchHit>,
    pub total_hits: i32,
    pub took_ms: i64,
}

// ── Query classification ──────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryKind {
    /// "JHN 3:16", "john 3", "gen 1:1-5", "1 cor 13"
    Reference {
        book: String,
        chapter: i32,
        verse: Option<i32>,
        end_verse: Option<i32>,
    },
    /// "H430", "G26"
    Strongs(String),
    /// "shepherd", "love of God", "ACAKI"
    Text(String),
}

/// Parse a user query into one of the three shapes.
///
/// Deliberately simple: it recognizes the forms users actually type
/// for Bible search. It does not try to be a general NLP parser,
/// and it never falls back to a "did you mean" guess — an
/// unparseable reference is treated as text search, which is what
/// the user expects when they typed a word.
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
///   "JHN 3:16", "john 3", "gen 1:1-5", "1 cor 13", "1cor13"
///
/// Book part is the leading alphabetic tokens (with optional leading
/// number); the rest is chapter[:verse[-endverse]]. Returns `None`
/// if the shape does not match — the caller then treats the query as
/// text search.
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

// ── Highlighting ──────────────────────────────────────────────

/// Find every occurrence of `query`'s tokens in `text`, case-
/// insensitively, and return byte spans. Multi-word queries return
/// spans for each token separately, sorted by position.
///
/// Tokenization mirrors `fts_sanitize`: split on whitespace, strip
/// surrounding quotes, lowercase. This is not a full Unicode
/// word-boundary search, but it matches what FTS5 indexed, which
/// is what the user sees highlighted.
///
/// Byte-offset caveat: `text.to_lowercase()` can change byte length
/// (ß → ss, İ → i̇). For ASCII and Latin-1 text — which covers BSB,
/// KJV, and ach_abr — lowercasing is byte-length-preserving and the
/// offsets are exact. For text with combining marks they may be off
/// by a byte or two, which is acceptable for highlighting. A
/// fully Unicode-correct version needs char-index mapping.
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

// ── Book resolution ───────────────────────────────────────────

/// Resolve a normalized book name (lowercase alphanumeric) against
/// the `books` table. Matches, in priority order:
///   1. exact `id` (case-insensitive) — "jhn"
///   2. exact `name` (case-insensitive, alnum-only) — "john"
///   3. `name` prefix — "joh" matches "John"
///
/// Ambiguity (e.g. "jo" matching "Job" and "John") is resolved by
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

// ── The search itself ─────────────────────────────────────────

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

// ── Text search (FTS5) ────────────────────────────────────────

/// Build one `SearchHit` from an FTS5 result row. The `query` is
/// the raw user text, used to compute highlight spans against the
/// joined `verses.text`.
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

    let fts_query = fts_sanitize(text);

    // Canonical book ID lists. Kept here so the frontend doesn't have
    // to know which translations use which book IDs — every installed
    // translation uses these same USFM-style IDs.
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

    // ── Build the dynamic WHERE clause ─────────────────────────────
    //
    // Bind order matters: ?1 is always the FTS query, then optional
    // translation_id, then the book list, then limit.
    let mut where_clauses = vec!["f.verses_fts MATCH ?1".to_string()];
    let mut bind_idx = 2;

    if translation_id.is_some() {
        where_clauses.push(format!("v.translation_id = ?{bind_idx}"));
        bind_idx += 1;
    }

    // Which book IDs to constrain to, if any. Explicit list wins over
    // the testament flags (a caller who passes both is asking for the
    // explicit list, and the testament flags become noise).
    let book_filter: Option<&[&str]> = if !filter.book_ids.is_empty() {
        // The caller passed an explicit list. We can't borrow from
        // `filter.book_ids` here because its type is `Vec<String>` and
        // we need `&[&str]`, so we take a different path below and
        // inline the values as parameters rather than a const slice.
        None
    } else if filter.old_testament_only {
        Some(OT_IDS)
    } else if filter.new_testament_only {
        Some(NT_IDS)
    } else {
        None
    };

    let explicit_books: Option<&[String]> = if !filter.book_ids.is_empty() {
        Some(&filter.book_ids)
    } else {
        None
    };

    // Emit the IN clause and reserve the parameter slots it needs.
    let num_book_params = if let Some(books) = explicit_books {
        books.len()
    } else if let Some(ids) = book_filter {
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

    // ── Hits query ─────────────────────────────────────────────────
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

    // ── Bind parameters dynamically ────────────────────────────────
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
    } else if let Some(ids) = book_filter {
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

    // ── Count query ────────────────────────────────────────────────
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

// ── Reference search ──────────────────────────────────────────

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

// ── Strong's search ───────────────────────────────────────────

fn search_strongs(
    conn: &Connection,
    strongs: &str,
    limit: i32,
) -> Result<(Vec<SearchHit>, i32), FreeBibleError> {
    // `lexicon` is not per-translation, so the translation filter
    // does not apply. One hit per lexicon entry.
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

// ── FTS5 query sanitization ───────────────────────────────────

/// Wrap every token in double quotes so user input cannot inject
/// FTS5 operators (AND, OR, NEAR, `*`, `:`). Multi-word queries
/// become AND-of-tokens, which is what "love God" should mean.
fn fts_sanitize(text: &str) -> String {
    text.split_whitespace()
        .map(|token| {
            let escaped = token.replace('"', "\"\"");
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ── Tests ─────────────────────────────────────────────────────

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

    #[test]
    fn fts_sanitize_quotes_every_token() {
        assert_eq!(fts_sanitize("love God"), "\"love\" \"God\"");
        assert_eq!(fts_sanitize("a\"b"), "\"a\"\"b\"");
    }
}
