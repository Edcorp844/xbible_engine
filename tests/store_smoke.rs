use xbible_engine::engines::freebible_engine::store_api::FreeBibleStore;

#[test]
#[ignore = "hits the live network"]
fn store_refresh_and_feed() {
    let store = FreeBibleStore::new();
    store.refresh().expect("refresh failed");

    let langs = store.languages();
    assert!(langs.len() > 100, "expected >100 languages, got {}", langs.len());

    let shelves = store.home_feed(None);
    assert!(!shelves.is_empty(), "home_feed returned no shelves");

    let results = store.search("king".into());
    assert!(!results.is_empty(), "search('king') returned nothing");
}