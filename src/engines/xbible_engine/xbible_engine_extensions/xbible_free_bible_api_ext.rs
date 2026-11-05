use crate::engines::freebible_engine::client::{
    FreeBibleBook, FreeBibleChapterItem, FreeBibleFootnote, FreeBibleVerse,
};
use crate::engines::freebible_engine::store::{LanguageFacet, StoreItem, StoreShelf};
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

    // ── Install / uninstall ──────────────────────────────────────

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

    // ── Read path ────────────────────────────────────────────────

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
}
