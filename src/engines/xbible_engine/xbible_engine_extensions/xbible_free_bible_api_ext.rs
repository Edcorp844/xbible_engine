use crate::engines::freebible_engine::EntitySummary;
use crate::engines::freebible_engine::client::{
    FreeBibleBook, FreeBibleChapterItem, FreeBibleFootnote, FreeBibleVerse,
};
use crate::engines::freebible_engine::store::{LanguageFacet, StoreItem, StoreShelf};
use crate::engines::freebible_engine::store_api::{
    EntityRef, EventDetail, GroupDetail, PersonDetail, PlaceDetail, VerseRange,
};
use crate::engines::freebible_engine::utils::{FreeBibleError, FreeBibleTaskStatus};
use crate::engines::xbible_engine::engine::XBibleEngine;

#[uniffi::export]
impl XBibleEngine {
    // ── Store: database setup ────────────────────────────────────

    /// Point the FreeBible store at a SQLite file. Call once at startup.
    pub fn set_free_bible_db_path(&self, path: String) -> Result<(), FreeBibleError> {
        self.free_bible_store.set_db_path(path)
    }

    // ── Store: catalog ───────────────────────────────────────────

    /// Fetch/refresh the FreeBible catalog (translations, commentaries, datasets).
    /// Falls back to SQLite if the network is unavailable.
    pub fn refresh_free_bible_catalog(&self) -> Result<(), FreeBibleError> {
        self.free_bible_store.refresh()
    }

    /// Home feed, App Store style. `language = None` shows everything.
    pub fn free_bible_home_feed(&self, language: Option<String>) -> Vec<StoreShelf> {
        self.free_bible_store.home_feed(language)
    }

    /// Search translations, commentaries, datasets.
    pub fn search_free_bible_store(&self, query: String) -> Vec<StoreItem> {
        self.free_bible_store.search(query)
    }

    /// Language list for the sidebar.
    pub fn free_bible_languages(&self) -> Vec<LanguageFacet> {
        self.free_bible_store.languages()
    }

    /// Detail for one item: kind is "translation" | "commentary" | "dataset".
    pub fn free_bible_store_item(&self, id: String, kind: String) -> Option<StoreItem> {
        self.free_bible_store.item(id, kind)
    }

    // ── Install / uninstall: translations ────────────────────────

    /// Start installing a translation. Returns a task ID for `poll_free_bible_task`.
    pub fn install_free_bible_translation(
        &self,
        translation_id: String,
    ) -> Result<String, FreeBibleError> {
        self.free_bible_store.install_translation(translation_id)
    }

    /// Start uninstalling a translation. Returns a task ID for `poll_free_bible_task`.
    pub fn uninstall_free_bible_translation(
        &self,
        translation_id: String,
    ) -> Result<String, FreeBibleError> {
        self.free_bible_store.uninstall_translation(translation_id)
    }

    pub fn is_free_bible_installed(&self, translation_id: String) -> bool {
        self.free_bible_store.is_installed(translation_id)
    }

    // ── Install: entity sets (people / events / places / groups) ─

    /// Start installing an entity set.
    /// `kind` is one of "person" | "event" | "place" | "group".
    /// Returns a task ID for `poll_free_bible_task`.
    pub fn install_free_bible_entity_set(
        &self,
        dataset_id: String,
        kind: String,
    ) -> Result<String, FreeBibleError> {
        self.free_bible_store.install_entity_set(dataset_id, kind)
    }

    pub fn is_free_bible_entity_set_installed(&self, dataset_id: String, kind: String) -> bool {
        self.free_bible_store
            .is_entity_set_installed(dataset_id, kind)
    }

    // ── Task polling (this is how the UI gets install progress) ──

    /// Poll a FreeBible task.
    ///
    /// While the task is `Running`, the returned status carries a full
    /// `InstallProgress`: fraction, chapters_done/total, verses_done,
    /// current book/chapter, and a human-readable message. Call this on a
    /// timer (250–500 ms is typical) and update your progress bar.
    ///
    /// Returns `None` if the task ID is unknown.
    pub fn poll_free_bible_task(&self, task_id: String) -> Option<FreeBibleTaskStatus> {
        self.free_bible_store.poll_task(task_id)
    }

    pub fn cancel_free_bible_task(&self, task_id: String) {
        self.free_bible_store.cancel_task(task_id)
    }

    pub fn list_free_bible_tasks(&self) -> Vec<FreeBibleTaskStatus> {
        self.free_bible_store.list_tasks()
    }

    // ── Read path: translation content ───────────────────────────

    pub fn free_bible_get_books(&self, translation_id: String) -> Vec<FreeBibleBook> {
        self.free_bible_store.get_books(translation_id)
    }

    pub fn free_bible_get_chapter(
        &self,
        translation_id: String,
        book_id: String,
        chapter: i32,
    ) -> Vec<FreeBibleVerse> {
        self.free_bible_store
            .get_chapter(translation_id, book_id, chapter)
    }

    pub fn free_bible_get_verse(
        &self,
        translation_id: String,
        book_id: String,
        chapter: i32,
        verse: i32,
    ) -> Option<String> {
        self.free_bible_store
            .get_verse(translation_id, book_id, chapter, verse)
    }

    pub fn free_bible_get_chapter_items(
        &self,
        translation_id: String,
        book_id: String,
        chapter: i32,
    ) -> Vec<FreeBibleChapterItem> {
        self.free_bible_store
            .get_chapter_items(translation_id, book_id, chapter)
    }

    pub fn free_bible_get_footnotes(
        &self,
        translation_id: String,
        book_id: String,
        chapter: i32,
    ) -> Vec<FreeBibleFootnote> {
        self.free_bible_store
            .get_footnotes(translation_id, book_id, chapter)
    }

    // ── Read path: entity detail ─────────────────────────────────

    pub fn free_bible_get_person(&self, dataset_id: String, id: String) -> Option<PersonDetail> {
        self.free_bible_store.get_person(dataset_id, id)
    }

    pub fn free_bible_get_event(&self, dataset_id: String, id: String) -> Option<EventDetail> {
        self.free_bible_store.get_event(dataset_id, id)
    }

    pub fn free_bible_get_place(&self, dataset_id: String, id: String) -> Option<PlaceDetail> {
        self.free_bible_store.get_place(dataset_id, id)
    }

    pub fn free_bible_get_group(&self, dataset_id: String, id: String) -> Option<GroupDetail> {
        self.free_bible_store.get_group(dataset_id, id)
    }

    // ── Read path: entity browse + search ────────────────────────

    pub fn free_bible_list_people(
        &self,
        dataset_id: String,
        offset: i32,
        limit: i32,
        query: Option<String>,
    ) -> Vec<EntitySummary> {
        self.free_bible_store
            .list_people(dataset_id, offset, limit, query)
    }

    pub fn free_bible_list_events(
        &self,
        dataset_id: String,
        offset: i32,
        limit: i32,
        query: Option<String>,
    ) -> Vec<EntitySummary> {
        self.free_bible_store
            .list_events(dataset_id, offset, limit, query)
    }

    pub fn free_bible_list_places(
        &self,
        dataset_id: String,
        offset: i32,
        limit: i32,
        query: Option<String>,
    ) -> Vec<EntitySummary> {
        self.free_bible_store
            .list_places(dataset_id, offset, limit, query)
    }

    pub fn free_bible_list_groups(
        &self,
        dataset_id: String,
        offset: i32,
        limit: i32,
        query: Option<String>,
    ) -> Vec<EntitySummary> {
        self.free_bible_store
            .list_groups(dataset_id, offset, limit, query)
    }

    pub fn free_bible_search_entities(
        &self,
        dataset_id: String,
        query: String,
    ) -> Vec<EntitySummary> {
        self.free_bible_store.search_entities(dataset_id, query)
    }

    // ── Read path: graph walks ───────────────────────────────────

    /// Every entity whose verse range covers `(book, chapter, verse)`.
    /// Powers "who's in this verse" in the reader.
    pub fn free_bible_entities_for_verse(
        &self,
        dataset_id: String,
        book: String,
        chapter: i32,
        verse: i32,
    ) -> Vec<EntityRef> {
        self.free_bible_store
            .get_entities_for_verse(dataset_id, book, chapter, verse)
    }

    /// All verse ranges for an entity.
    pub fn free_bible_verse_refs(
        &self,
        dataset_id: String,
        kind: String,
        id: String,
    ) -> Vec<VerseRange> {
        self.free_bible_store.get_verse_refs(dataset_id, kind, id)
    }

    /// All entities an entity points to, filtered by relation.
    /// E.g. `relation = Some("participant")` on an event gives its participants;
    /// `relation = None` gives every outgoing edge.
    pub fn free_bible_related_entities(
        &self,
        dataset_id: String,
        kind: String,
        id: String,
        relation: Option<String>,
    ) -> Vec<EntityRef> {
        self.free_bible_store
            .get_related(dataset_id, kind, id, relation)
    }
}
