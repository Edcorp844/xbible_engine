use log::{error, info};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use xbible_engine::engines::xbible_engine::engine::XBibleEngine;

#[derive(Clone, Debug, PartialEq)]
enum EngineStep {
    Initializing,
    FetchingSources {
        current: usize,
        total: usize,
        active_source: String,
    },
    ProcessingSelection,
    InstallingModules,
    Failed(String),
    Finished,
}

#[derive(Clone, Debug)]
struct InstallTracker {
    module_name: String,
    progress: f32,
    status: String,  // "Queued" | "downloading" | "extracting" | "complete" | "error"
    message: String, // live text from SWORD, e.g. "Downloading nt.bzs"
}

struct AppState {
    current_step: EngineStep,
    installers: Vec<InstallTracker>,
    global_progress: f32,
}

fn main() {
    info!("🚀 Starting Engine Monitor Context...");

    xbible_engine::init_logging();

    let engine = Arc::new(XBibleEngine::new());

    let state = Arc::new(Mutex::new(AppState {
        current_step: EngineStep::Initializing,
        installers: Vec::new(),
        global_progress: 0.0,
    }));

    // ─────────────────────────────────────────────────────────────────────────
    //  BACKGROUND WORKER
    // ─────────────────────────────────────────────────────────────────────────
    let engine_worker = Arc::clone(&engine);
    let state_worker = Arc::clone(&state);

    thread::spawn(move || {
        let set_step = |step: EngineStep| {
            if let Ok(mut s) = state_worker.lock() {
                s.current_step = step;
            }
        };

        // 1. Fetch Sources
        let sources = engine_worker.get_remote_sources();
        let total_sources = sources.len();

        if sources.is_empty() {
            set_step(EngineStep::Failed(
                "No remote sources are configured.".to_string(),
            ));
            return;
        }

        info!("📡 Found {} remote sources", total_sources);

        // 2. Fetch module catalogs SEQUENTIALLY (SWORD is not thread-safe)
        let mut remote_modules = Vec::new();

        for (idx, source) in sources.into_iter().enumerate() {
            let source_identifier = format!("{:?}", source);

            // Update UI immediately so the monitor shows progress
            if let Ok(mut s) = state_worker.lock() {
                s.current_step = EngineStep::FetchingSources {
                    current: idx + 1,
                    total: total_sources,
                    active_source: source_identifier.clone(),
                };
                s.global_progress = ((idx + 1) as f32) / (total_sources as f32);
            }

            info!("📡 Fetching modules from {} ...", source_identifier);

            let mut modules = engine_worker.fetch_remote_modules(&source);
            if !modules.is_empty() {
                remote_modules.append(&mut modules);
            }
        }

        if remote_modules.is_empty() {
            set_step(EngineStep::Failed(
                "Network aggregation returned zero remote modules.".to_string(),
            ));
            return;
        }

        info!("📦 Aggregated {} remote modules", remote_modules.len());

        // 3. Selection — ONE module
        set_step(EngineStep::ProcessingSelection);
        thread::sleep(Duration::from_millis(400));

        let selected: Option<(String, String)> = remote_modules
            .iter()
            .filter(|m| !engine_worker.is_module_installed(&m.name))
            .map(|m| (m.name.clone(), m.source.clone()))
            .next();

        let Some((module_name, module_source)) = selected else {
            set_step(EngineStep::Failed(
                "No uninstalled modules available to test. Delete one from ~/…/Modules first."
                    .to_string(),
            ));
            return;
        };

        info!(
            "🎯 Selected module to install: {} (source: {})",
            module_name, module_source
        );

        {
            if let Ok(mut s) = state_worker.lock() {
                s.installers = vec![InstallTracker {
                    module_name: module_name.clone(),
                    progress: 0.0,
                    status: "Queued".to_string(),
                    message: String::new(),
                }];
                s.current_step = EngineStep::InstallingModules;
            }
        }

        // 4. REAL install
        {
            if let Ok(mut s) = state_worker.lock() {
                s.installers[0].status = "Installing".to_string();
            }
        }

                info!("⬇️  Calling engine.install_module({}) ...", module_name);
        let start = std::time::Instant::now();

        // Poller
        let engine_poll = Arc::clone(&engine_worker);
        let state_poll = Arc::clone(&state_worker);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_poll = Arc::clone(&stop);

        let poll_handle = thread::spawn(move || {
            while !stop_poll.load(Ordering::Relaxed) {
                let details = engine_poll.get_download_progress_details();

                if let Ok(mut s) = state_poll.lock() {
                    if !s.installers.is_empty() {
                        s.installers[0].progress = details.progress as f32;
                        s.installers[0].status = details.status.clone();
                        s.installers[0].message = if details.message.is_empty() {
                            format!("Downloading {}…", s.installers[0].module_name)
                        } else {
                            details.message.clone()
                        };
                    }
                }
                thread::sleep(Duration::from_millis(80));
            }
        });

        // Blocking install (this is where rebuild_mgr runs on success)
        let result = engine_worker.install_module(&module_source, &module_name);

        // Stop poller ASAP – do not let it race with the just-finished rebuild
        stop.store(true, Ordering::Relaxed);
        let _ = poll_handle.join();

        let elapsed = start.elapsed();
        info!("⬇️  install_module returned {} in {:?}", result, elapsed);

        // Force final UI state (prevents the 0% / “waiting” flicker)
        let actually_installed = engine_worker.is_module_installed(&module_name);

        if let Ok(mut s) = state_worker.lock() {
            if !s.installers.is_empty() {
                if actually_installed {
                    s.installers[0].status = "complete".to_string();
                    s.installers[0].progress = 1.0;
                    s.installers[0].message = "Installation finished".to_string();
                } else {
                    s.installers[0].status = "error".to_string();
                    s.installers[0].message = format!("Install failed (code {})", result);
                }
            }
        }

        set_step(EngineStep::Finished);
    });

    // ─────────────────────────────────────────────────────────────────────────
    //  MONITOR LOOP
    // ─────────────────────────────────────────────────────────────────────────
    loop {
        let current_state = match state.lock() {
            Ok(guard) => guard,
            Err(_) => continue,
        };

        // Clear screen
        info!("{}[2J{}[H", 27 as char, 27 as char);
        info!("🛰️  === ENGINE PROCESS SYNCHRONIZATION MONITOR ===");
        info!("--------------------------------------------------");

        match &current_state.current_step {
            EngineStep::Initializing => {
                info!("⏳ Initializing...");
            }
            EngineStep::FetchingSources {
                current,
                total,
                active_source,
            } => {
                info!("📡 STATUS: Fetching Remote Source Catalogs");
                info!("📦 Progress: Source [{}/{}]", current, total);
                info!("🔍 Active source: {}", active_source);
                info!("   Total Sync Progress:");
                render_progress_bar(current_state.global_progress);
            }
            EngineStep::ProcessingSelection => {
                info!("📦 STATUS: Selecting module to install...");
            }
            EngineStep::InstallingModules => {
                info!("📥 STATUS: Installing Module Packages");
                info!("--------------------------------------------------");
                for tracker in current_state.installers.iter() {
                    info!(
                        "📍 Module: {:<14} | Status: {:<12}",
                        tracker.module_name, tracker.status
                    );
                    if !tracker.message.is_empty() {
                        info!("   💬 {}", tracker.message);
                    }
                    render_progress_bar(tracker.progress);
                }
            }
            EngineStep::Failed(err_message) => {
                error!("❌ PIPELINE ERROR DETECTED: {}", err_message);
                break;
            }
            EngineStep::Finished => {
                info!("🎉 DONE.");
                let available_modules = engine.get_available_modules();
                info!("Available Modules now are:");
                info!("{:?}", available_modules);
                break;
            }
        }

        drop(current_state);
        thread::sleep(Duration::from_millis(100));
    }
}

fn render_progress_bar(progress: f32) {
    let bar_width: i32 = 20;
    let filled_width = (progress * bar_width as f32).round() as i32;
    let empty_width = bar_width.saturating_sub(filled_width);

    info!(
        "[{}{}] {:.1}%",
        "█".repeat(filled_width as usize),
        "-".repeat(empty_width as usize),
        progress * 100.0
    );
}