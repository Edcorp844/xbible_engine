use std::thread;
use std::time::Duration;

use xbible_engine::engines::freebible_engine::FreeBibleTaskState;
use xbible_engine::XBibleEngine;

fn main() {
    let db_path = std::env::temp_dir().join("install_persistence.db");
    cleanup(&db_path);

    let engine = XBibleEngine::new();
    engine
        .freebible()
        .set_db_path(db_path.to_string_lossy().into_owned())
        .expect("open db");

    let store = engine.freebible().store();
    let reader = engine.freebible().reader();

    // ── Step 1: refresh, then check DB state ────────────────────
    println!("[1] Refreshing catalog…");
    store.refresh().expect("refresh");

    let rows_before = dump_translations(&db_path);
    println!("    rows in translations: {}", rows_before.len());
    for r in &rows_before {
        if r.0 == "BSB" {
            println!("    BSB installed BEFORE install: {}", r.1);
        }
    }

    // ── Step 2: install BSB ─────────────────────────────────────
    println!("\n[2] Installing BSB…");
    let task = store.install_translation("BSB".into()).expect("install");
    wait_for_task(&store, &task);

    let rows_after_install = dump_translations(&db_path);
    let bsb_after_install = rows_after_install
        .iter()
        .find(|r| r.0 == "BSB")
        .map(|r| r.1)
        .unwrap_or(-1);
    println!("    BSB installed AFTER install: {}", bsb_after_install);
    assert_eq!(bsb_after_install, 1, "install did not set installed=1!");

    let installed_via_reader = reader
        .is_installed("BSB".into())
        .expect("is_installed failed");
    println!("    reader.is_installed(BSB): {}", installed_via_reader);
    assert!(installed_via_reader, "reader.is_installed returned false!");

    let list_before_refresh = reader.get_installed_translations();
    println!(
        "    reader.get_installed_translations: {} entries",
        list_before_refresh.len()
    );
    assert_eq!(
        list_before_refresh.len(),
        1,
        "expected exactly 1 installed translation, got {}",
        list_before_refresh.len()
    );

    // ── Step 3: THE critical step — refresh again ───────────────
    println!("\n[3] Refreshing catalog again (this is what your app does on relaunch)…");
    store.refresh().expect("refresh again");

    let rows_after_refresh = dump_translations(&db_path);
    let bsb_after_refresh = rows_after_refresh
        .iter()
        .find(|r| r.0 == "BSB")
        .map(|r| r.1)
        .unwrap_or(-1);
    println!("    BSB installed AFTER second refresh: {}", bsb_after_refresh);

    if bsb_after_refresh == 0 {
        println!("\n    ✗ ✗ ✗ BUG CONFIRMED: refresh() wiped installed=1");
        println!("    The upsert in save_translations is the culprit.");
        std::process::exit(1);
    } else {
        println!("    ✓ installed=1 survived refresh");
    }

    let list_after_refresh = reader.get_installed_translations();
    println!(
        "    reader.get_installed_translations: {} entries",
        list_after_refresh.len()
    );
    assert_eq!(
        list_after_refresh.len(),
        1,
        "get_installed_translations returned {} after refresh",
        list_after_refresh.len()
    );

    println!("\nDone. All invariants held.");
    cleanup(&db_path);
}

fn wait_for_task(
    store: &std::sync::Arc<xbible_engine::engines::freebible_engine::store_api::FreeBibleStore>,
    task_id: &str,
) {
    let mut last_pct = -1.0f64;
    loop {
        thread::sleep(Duration::from_millis(300));
        let Some(status) = store.poll_task(task_id.to_string()) else {
            println!("    task disappeared");
            break;
        };
        let pct = status.progress.fraction * 100.0;
        if pct - last_pct >= 10.0 || pct >= 100.0 {
            last_pct = pct;
            println!("    [{:5.1}%] {}", pct, status.progress.message);
        }
        match status.state {
            FreeBibleTaskState::Running | FreeBibleTaskState::Queued => continue,
            FreeBibleTaskState::Completed => break,
            FreeBibleTaskState::Failed { error } => {
                panic!("install failed: {error}");
            }
            FreeBibleTaskState::Cancelled => panic!("install cancelled"),
        }
    }
}

fn dump_translations(path: &std::path::Path) -> Vec<(String, i32)> {
    let conn = rusqlite::Connection::open(path).unwrap();
    let mut stmt = conn
        .prepare("SELECT id, installed FROM translations ORDER BY id")
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i32>(1)?)))
        .unwrap();
    rows.filter_map(|r| r.ok()).collect()
}

fn cleanup(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{}-wal", path.to_string_lossy()));
    let _ = std::fs::remove_file(format!("{}-shm", path.to_string_lossy()));
}