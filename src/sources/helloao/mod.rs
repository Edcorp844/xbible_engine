//! HelloAO source adapter (`src/sources/helloao/mod.rs`).
//!
//! Wraps `HelloAOClient` and implements `BibleSource` so `helloao` endpoint
//! details, raw JSON parsing, and API quirks remain isolated behind normalized
//! `Generic*` structures, while providing explicit installation and
//! uninstallation methods for HelloAO translations.

pub mod catalog;
pub mod client;
pub mod values;

use rusqlite::Connection;

use crate::db::install::{persist_installed_bible, uninstall_translation};
use crate::error::XBibleError;
use crate::sources::helloao::client::{
    HELLOAO_SOURCE_ID, HelloAOClient, HelloCommentaryMeta, HelloCommentaryProfile,
    HelloDatasetMeta, HelloEntitySummary, HelloLexiconEntry,
};
use crate::sources::{
    BibleSource, GenericBook, GenericCatalogBible, GenericChapter, GenericInstalledBible,
    InstallProgressSink,
};

/// Adapter exposing HelloAO data and installation operations through `BibleSource`
/// and source-specific helpers.
#[derive(Debug, Clone)]
pub struct HelloAOSource {
    client: HelloAOClient,
}

impl HelloAOSource {
    /// Creates a new `HelloAOSource` using the default HelloAO endpoint.
    pub fn new() -> Self {
        let client: HelloAOClient = HelloAOClient::new();
        Self { client }
    }

    /// Creates a new `HelloAOSource` with a custom base URL (useful for tests
    /// or alternative mirrors).
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        let client: HelloAOClient = HelloAOClient::with_base_url(base_url);
        Self { client }
    }

    /// Returns a reference to the underlying `HelloAOClient` for source-specific
    /// operations (such as HelloAO commentaries, profiles, or datasets).
    pub fn client(&self) -> &HelloAOClient {
        &self.client
    }

    // ── Translation Install & Uninstall ───────────────────────────────────────

    /// Downloads, normalizes, and persists a HelloAO translation into SQLite.
    pub fn install_translation(
        &self,
        conn: &Connection,
        translation_id: &str,
        progress: &dyn InstallProgressSink,
    ) -> Result<GenericInstalledBible, XBibleError> {
        let bundle: GenericInstalledBible = self
            .fetch_for_install(translation_id, progress)
            .map_err(|msg: String| XBibleError::Generic { error_message: msg })?;
        persist_installed_bible(conn, &bundle)?;
        Ok(bundle)
    }

    /// Removes installed rows for a HelloAO translation and clears its `installed` flag.
    pub fn uninstall_translation(
        &self,
        conn: &Connection,
        translation_id: &str,
    ) -> Result<(), XBibleError> {
        uninstall_translation(conn, translation_id)
    }

    // ── Source-Specific Helpers (HelloAO Commentaries & Datasets) ────────────

    /// Lists available HelloAO commentaries.
    pub fn commentaries(&self) -> Result<Vec<HelloCommentaryMeta>, String> {
        let list: Vec<HelloCommentaryMeta> = self.client.commentaries()?;
        Ok(list)
    }

    /// Lists books for a HelloAO commentary normalized into `GenericBook`.
    pub fn commentary_books(&self, commentary_id: &str) -> Result<Vec<GenericBook>, String> {
        let books: Vec<GenericBook> = self.client.commentary_books(commentary_id)?;
        Ok(books)
    }

    /// Fetches a single HelloAO commentary chapter normalized into `GenericChapter`.
    pub fn commentary_chapter(
        &self,
        commentary_id: &str,
        book_id: &str,
        chapter: u32,
    ) -> Result<GenericChapter, String> {
        let ch: GenericChapter = self
            .client
            .commentary_chapter(commentary_id, book_id, chapter)?;
        Ok(ch)
    }

    /// Lists available profiles for a HelloAO commentary.
    pub fn commentary_profiles(
        &self,
        commentary_id: &str,
    ) -> Result<Vec<HelloCommentaryProfile>, String> {
        let profiles: Vec<HelloCommentaryProfile> =
            self.client.commentary_profiles(commentary_id)?;
        Ok(profiles)
    }

    /// Fetches a specific profile for a HelloAO commentary.
    pub fn commentary_profile(
        &self,
        commentary_id: &str,
        profile_id: &str,
    ) -> Result<HelloCommentaryProfile, String> {
        let profile: HelloCommentaryProfile =
            self.client.commentary_profile(commentary_id, profile_id)?;
        Ok(profile)
    }

    /// Lists available HelloAO datasets.
    pub fn datasets(&self) -> Result<Vec<HelloDatasetMeta>, String> {
        let list: Vec<HelloDatasetMeta> = self.client.datasets()?;
        Ok(list)
    }

    /// Lists people entities for a HelloAO dataset.
    pub fn dataset_people_list(&self, dataset_id: &str) -> Result<Vec<HelloEntitySummary>, String> {
        let list: Vec<HelloEntitySummary> = self.client.dataset_people_list(dataset_id)?;
        Ok(list)
    }

    /// Lists event entities for a HelloAO dataset.
    pub fn dataset_events_list(&self, dataset_id: &str) -> Result<Vec<HelloEntitySummary>, String> {
        let list: Vec<HelloEntitySummary> = self.client.dataset_events_list(dataset_id)?;
        Ok(list)
    }

    /// Lists place entities for a HelloAO dataset.
    pub fn dataset_places_list(&self, dataset_id: &str) -> Result<Vec<HelloEntitySummary>, String> {
        let list: Vec<HelloEntitySummary> = self.client.dataset_places_list(dataset_id)?;
        Ok(list)
    }

    /// Lists group entities for a HelloAO dataset.
    pub fn dataset_groups_list(&self, dataset_id: &str) -> Result<Vec<HelloEntitySummary>, String> {
        let list: Vec<HelloEntitySummary> = self.client.dataset_groups_list(dataset_id)?;
        Ok(list)
    }

    /// Fetches raw JSON for a single person entity in a HelloAO dataset.
    pub fn dataset_person(
        &self,
        dataset_id: &str,
        entity_id: &str,
    ) -> Result<serde_json::Value, String> {
        let val: serde_json::Value = self.client.dataset_person(dataset_id, entity_id)?;
        Ok(val)
    }

    /// Fetches raw JSON for a single event entity in a HelloAO dataset.
    pub fn dataset_event(
        &self,
        dataset_id: &str,
        entity_id: &str,
    ) -> Result<serde_json::Value, String> {
        let val: serde_json::Value = self.client.dataset_event(dataset_id, entity_id)?;
        Ok(val)
    }

    /// Fetches raw JSON for a single place entity in a HelloAO dataset.
    pub fn dataset_place(
        &self,
        dataset_id: &str,
        entity_id: &str,
    ) -> Result<serde_json::Value, String> {
        let val: serde_json::Value = self.client.dataset_place(dataset_id, entity_id)?;
        Ok(val)
    }

    /// Fetches raw JSON for a single group entity in a HelloAO dataset.
    pub fn dataset_group(
        &self,
        dataset_id: &str,
        entity_id: &str,
    ) -> Result<serde_json::Value, String> {
        let val: serde_json::Value = self.client.dataset_group(dataset_id, entity_id)?;
        Ok(val)
    }

    /// Fetches a lexicon entry by Strong's code from a HelloAO dataset.
    pub fn lexicon_entry(
        &self,
        dataset_id: &str,
        strongs: &str,
    ) -> Result<Option<HelloLexiconEntry>, String> {
        let entry: Option<HelloLexiconEntry> = self.client.lexicon_entry(dataset_id, strongs)?;
        Ok(entry)
    }
}

impl Default for HelloAOSource {
    fn default() -> Self {
        Self::new()
    }
}

impl BibleSource for HelloAOSource {
    fn source_id(&self) -> &str {
        HELLOAO_SOURCE_ID
    }

    fn display_name(&self) -> &str {
        "HelloAO Bible API"
    }

    fn list_catalog(&self) -> Result<Vec<GenericCatalogBible>, String> {
        self.client.list_catalog()
    }

    fn list_books(&self, bible_id: &str) -> Result<Vec<GenericBook>, String> {
        self.client.list_books(bible_id)
    }

    fn fetch_chapter(
        &self,
        bible_id: &str,
        book_id: &str,
        chapter_number: i32,
    ) -> Result<GenericChapter, String> {
        self.client.fetch_chapter(bible_id, book_id, chapter_number)
    }

    fn fetch_for_install(
        &self,
        bible_id: &str,
        progress: &dyn InstallProgressSink,
    ) -> Result<GenericInstalledBible, String> {
        self.client.fetch_for_install(bible_id, progress)
    }
}
