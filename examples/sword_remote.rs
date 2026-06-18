use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use xbible_engine::engines::xbible_engine::engine::XBibleEngine;

#[derive(Clone, Debug, PartialEq)]
enum EngineStep {
    Initializing,
    FetchingSources { current: usize, total: usize, active_source: String },
    ProcessingSelection,
    InstallingModules,
    Failed(String),
    Finished,
}

struct InstallTracker {
    module_name: String,
    progress: f32,
    status: String,
}

struct AppState {
    current_step: EngineStep,
    installers: Vec<InstallTracker>,
    global_progress: f32,
}

fn main() {
    println!("🚀 Starting Engine Monitor Context...");
    
    // Initialize the engine core safely
    let engine = Arc::new(XBibleEngine::new());
    
    let state = Arc::new(Mutex::new(AppState {
        current_step: EngineStep::Initializing,
        installers: Vec::new(),
        global_progress: 0.0,
    }));

    // ─────────────────────────────────────────────────────────────────────────
    //  BACKGROUND WORKER MANAGING THREAD
    // ─────────────────────────────────────────────────────────────────────────
    let engine_worker = Arc::clone(&engine);
    let state_worker = Arc::clone(&state);

    thread::spawn(move || {
        let set_step = |step: EngineStep| {
            if let Ok(mut s) = state_worker.lock() {
                s.current_step = step;
            }
        };

        // 1. Fetch Remote Sources List (Local structural read)
        let sources = engine_worker.get_remote_sources();
        let total_sources = sources.len();

        if sources.is_empty() {
            set_step(EngineStep::Failed("No remote sources are configured in the engine context.".to_string()));
            return;
        }

        // Shared aggregators for concurrent network fetching tasks
        let remote_modules_aggr = Arc::new(Mutex::new(Vec::new()));
        let completed_sources = Arc::new(AtomicUsize::new(0));
        let mut fetch_handles = vec![];

        // 2. Spawn Concurrency Workers to pull remote catalogs in parallel
        for source in sources {
            let engine_cp = Arc::clone(&engine_worker);
            let modules_cp = Arc::clone(&remote_modules_aggr);
            let completed_cp = Arc::clone(&completed_sources);
            let state_cp = Arc::clone(&state_worker);
            
            // Adjust `.name` or formatting depending on what your Engine Source struct fields look like
            let source_identifier = format!("{:?}", source); 

            let handle = thread::spawn(move || {
                // Network network bound sync operation (Runs concurrently across cores)
                let mut modules = engine_cp.fetch_remote_modules(&source);
                
                if !modules.is_empty() {
                    if let Ok(mut master_list) = modules_cp.lock() {
                        master_list.append(&mut modules);
                    }
                }

                // Increment tracker safely using atomic hardware instructions
                let finished = completed_cp.fetch_add(1, Ordering::SeqCst) + 1;

                // Push dynamic diagnostic frame up to the UI thread
                if let Ok(mut s) = state_cp.lock() {
                    s.current_step = EngineStep::FetchingSources {
                        current: finished,
                        total: total_sources,
                        active_source: source_identifier,
                    };
                    s.global_progress = (finished as f32) / (total_sources as f32);
                }
            });

            fetch_handles.push(handle);
        }

        // Wait for all concurrent background connections to resolve completely
        for handle in fetch_handles {
            let _ = handle.join();
        }

        // Safely extract the compiled vector from the synchronization guard shell
        let mut remote_modules = match Arc::try_unwrap(remote_modules_aggr) {
            Ok(mutex) => mutex.into_inner().unwrap_or_default(),
            Err(mutex) => mutex.lock().unwrap().clone(),
        };

        if remote_modules.is_empty() {
            set_step(EngineStep::Failed("Network aggregation returned zero remote modules across all sources.".to_string()));
            return;
        }

        // 3. Selection Stage
        set_step(EngineStep::ProcessingSelection);
        thread::sleep(Duration::from_millis(600));

        let target_count = std::cmp::min(3, remote_modules.len());
        let selected_modules: Vec<String> = remote_modules
            .iter()
            .take(target_count)
            .map(|m| m.name.clone()) 
            .collect();

        {
            if let Ok(mut s) = state_worker.lock() {
                s.installers = selected_modules
                    .iter()
                    .map(|name| InstallTracker {
                        module_name: name.clone(),
                        progress: 0.0,
                        status: "Queued".to_string(),
                    })
                    .collect();
                s.current_step = EngineStep::InstallingModules;
            }
        }

        // 4. Sequential Module Install Sequence
        for i in 0..target_count {
            {
                if let Ok(mut s) = state_worker.lock() {
                    s.installers[i].status = "Installing".to_string();
                }
            }

            // Real execution binding placement hook: 
            // engine_worker.install_module(&selected_modules[i]);
            for progress_tick in 1..=100 {
                thread::sleep(Duration::from_millis(25));
                
                if let Ok(mut s) = state_worker.lock() {
                    s.installers[i].progress = progress_tick as f32 / 100.0;
                }
            }

            if let Ok(mut s) = state_worker.lock() {
                s.installers[i].status = "Completed".to_string();
                s.installers[i].progress = 1.0;
            }
        }

        set_step(EngineStep::Finished);
    });

    // ─────────────────────────────────────────────────────────────────────────
    //  MAIN UI RENDER LOOP (Main Thread Execution)
    // ─────────────────────────────────────────────────────────────────────────
    loop {
        let current_state = match state.lock() {
            Ok(guard) => guard,
            Err(_) => continue, // Avoid freezing if the background thread has an FFI issue
        };

        // Clear terminal screen completely
        print!("{}[2J{}[H", 27 as char, 27 as char);
        println!("🛰️  === ENGINE PROCESS SYNCHRONIZATION MONITOR ===");
        println!("--------------------------------------------------");

        match &current_state.current_step {
            EngineStep::Initializing => {
                println!("⏳ Hooking FFI bindings and initializing network subsystems...");
            }
            EngineStep::FetchingSources { current, total, active_source } => {
                println!("📡 STATUS: Fetching Remote Source Catalogs (Parallel)");
                println!("📦 Progress: Source [{}/{}]", current, total);
                println!("🔍 Last active worker read: {}", active_source);
                print!("   Total Sync Progress: ");
                render_progress_bar(current_state.global_progress);
            }
            EngineStep::ProcessingSelection => {
                println!("📦 STATUS: Unifying Distributed Module Inventories...");
                println!("🧬 Selecting target testing payloads safely...");
            }
            EngineStep::InstallingModules => {
                println!("📥 STATUS: Installing Module Packages");
                println!("--------------------------------------------------");
                for tracker in current_state.installers.iter() {
                    print!("📍 Module: {:<14} | Status: {:<10} | ", tracker.module_name, tracker.status);
                    render_progress_bar(tracker.progress);
                }
            }
            EngineStep::Failed(err_message) => {
                println!("❌ PIPELINE ERROR DETECTED");
                println!("⚠️ Reason: {}", err_message);
                break;
            }
            EngineStep::Finished => {
                println!("🎉 SUCCESS: All remote engine tasks executed with 0 crashes.");
                break;
            }
        }

        drop(current_state);
        thread::sleep(Duration::from_millis(100));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
//  Explicit Typings Progress Bar Utility
// ─────────────────────────────────────────────────────────────────────────────
fn render_progress_bar(progress: f32) {
    let bar_width: i32 = 20;
    let filled_width = (progress * bar_width as f32).round() as i32;
    let empty_width = bar_width.saturating_sub(filled_width);
    
    println!(
        "[{}{}] {:.1}%",
        "█".repeat(filled_width as usize),
        "-".repeat(empty_width as usize),
        progress * 100.0
    );
}