// freebible_engine/values.rs

use std::fmt;

// ── Counts ──────────────────────────────────────────────────────

macro_rules! count_type {
    ($name:ident, $what:literal) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(i32);

        impl $name {
            pub const ZERO: Self = Self(0);

            /// Clamp to zero — negative counts from a bad API payload
            /// become 0 rather than silently producing -1 in the UI.
            pub fn from_raw(n: i32) -> Self {
                Self(n.max(0))
            }

            pub fn raw(self) -> i32 {
                self.0
            }
            pub fn is_empty(self) -> bool {
                self.0 == 0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl std::ops::Add for $name {
            type Output = Self;
            fn add(self, rhs: Self) -> Self {
                Self(self.0 + rhs.0)
            }
        }
    };
}

count_type!(BookCount, "books");
count_type!(ChapterCount, "chapters");
count_type!(VerseCount, "verses");

// ── Booleans that come from integers on the wire ────────────────

/// `installed` stored as 0/1 in SQLite. Never appears as an i32 in
/// application code — only `Flag::from_sql_int` / `Flag::as_sql_int`
/// at the persistence boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flag {
    Off,
    On,
}

impl Flag {
    pub fn from_bool(b: bool) -> Self {
        if b { Flag::On } else { Flag::Off }
    }

    /// Read from SQLite INTEGER. Any non-zero is On.
    pub fn from_sql_int(n: i32) -> Self {
        if n != 0 { Flag::On } else { Flag::Off }
    }

    pub fn as_sql_int(self) -> i32 {
        match self {
            Flag::Off => 0,
            Flag::On => 1,
        }
    }

    pub fn as_bool(self) -> bool {
        matches!(self, Flag::On)
    }
}

// ── License info (paired fields that shouldn't be swapped) ──────

/// Groups `license_url` + `license_notes` so a call site can't
/// accidentally pass the URL where the notes go.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct License {
    pub url: Option<LicenseUrl>,
    pub notes: Option<LicenseNotes>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LicenseUrl(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LicenseNotes(pub String);

// ── Language code ───────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LanguageCode(pub String);

impl LanguageCode {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume and return the inner `String`. Used at the FFI
    /// boundary in `From<DomainStoreItem> for StoreItem`.
    pub fn into_inner(self) -> String {
        self.0
    }
}

// ── Text direction (closed set, exhaustive) ─────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDirection {
    Ltr,
    Rtl,
}

impl TextDirection {
    pub fn from_wire(s: &str) -> Self {
        match s {
            "rtl" | "RTL" => Self::Rtl,
            // Any unknown / empty value falls back to LTR — matches the
            // API's historical behavior. If you want to surface malformed
            // payloads, change this to `Result<Self, _>`.
            _ => Self::Ltr,
        }
    }

    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Ltr => "ltr",
            Self::Rtl => "rtl",
        }
    }
}

// ── Raw JSON payload ────────────────────────────────────────────

/// The verbatim JSON blob from the remote API. Kept as a distinct
/// type so it can never be confused with a description, a name, or
/// any other String field in a database row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawJson(pub String);

impl RawJson {
    pub fn empty() -> Self {
        Self(String::new())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

// ── Store item identity ─────────────────────────────────────────

/// Which kind of item a `StoreItem` represents. Closed set —
/// exhaustive `match` required everywhere it's used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StoreItemKind {
    Translation,
    Commentary,
    Dataset,
    EntitySet,
}

impl StoreItemKind {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Translation => "translation",
            Self::Commentary => "commentary",
            Self::Dataset => "dataset",
            Self::EntitySet => "entity_set",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "translation" => Some(Self::Translation),
            "commentary" => Some(Self::Commentary),
            "dataset" => Some(Self::Dataset),
            "entity_set" => Some(Self::EntitySet),
            _ => None,
        }
    }
}

/// Identity for a store item. Uniquely identifies any item in the
/// catalog across all kinds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StoreItemId {
    pub kind: StoreItemKind,
    pub raw: String,
}

impl StoreItemId {
    pub fn translation(id: impl Into<String>) -> Self {
        Self {
            kind: StoreItemKind::Translation,
            raw: id.into(),
        }
    }
    pub fn commentary(id: impl Into<String>) -> Self {
        Self {
            kind: StoreItemKind::Commentary,
            raw: id.into(),
        }
    }
    pub fn dataset(id: impl Into<String>) -> Self {
        Self {
            kind: StoreItemKind::Dataset,
            raw: id.into(),
        }
    }
    pub fn entity_set(id: impl Into<String>) -> Self {
        Self {
            kind: StoreItemKind::EntitySet,
            raw: id.into(),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

// ── Shelf identity ──────────────────────────────────────────────

/// Closed set of shelves the home feed can produce. Every shelf the
/// store returns must be one of these — forces exhaustive handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    pub fn title(self) -> &'static str {
        match self {
            Self::Featured => "Featured",
            Self::BibleReference => "Bible Reference",
            Self::WordStudy => "Word Study",
            Self::PublicDomain => "Public Domain",
            Self::Commentaries => "Commentaries",
            Self::StudyTools => "Study Tools",
            Self::AllResources => "All Resources",
        }
    }

    pub fn subtitle(self) -> Option<&'static str> {
        match self {
            Self::Featured => Some("Hand-picked for study"),
            Self::BibleReference => Some("People, places, and events from Scripture"),
            Self::WordStudy => Some("Original-language alignment with Strong's"),
            Self::PublicDomain => Some("Free to use, no strings attached"),
            Self::Commentaries => Some("Verse-by-verse explanations and insights"),
            Self::StudyTools => Some("Lexicons, cross-references, and more"),
            Self::AllResources => None, // computed at build time
        }
    }

    pub fn from_title(s: &str) -> Option<Self> {
        match s {
            "Featured" => Some(Self::Featured),
            "Bible Reference" => Some(Self::BibleReference),
            "Word Study" => Some(Self::WordStudy),
            "Public Domain" => Some(Self::PublicDomain),
            "Commentaries" => Some(Self::Commentaries),
            "Study Tools" => Some(Self::StudyTools),
            "All Resources" => Some(Self::AllResources),
            _ => None,
        }
    }

    pub const ALL: [ShelfId; 7] = [
        Self::Featured,
        Self::BibleReference,
        Self::WordStudy,
        Self::PublicDomain,
        Self::Commentaries,
        Self::StudyTools,
        Self::AllResources,
    ];
}

// values.rs (append)

/// Identifier for an install/uninstall task. Distinct type so
/// `cancelTask(taskId)` can't be called with a raw translation ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskId(pub String);

impl TaskId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl From<String> for TaskId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Which kind of async task is running. Closed set — exhaustive
/// match required everywhere it's used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskKind {
    Install,
    Uninstall,
    Entity,
}

impl TaskKind {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Uninstall => "uninstall",
            Self::Entity => "entity",
        }
    }
}
