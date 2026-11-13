use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use log::{error, info};
use rusqlite::Connection;

use crate::engines::freebible_engine::error::FreeBibleError;

const QUEUE_CAPACITY: usize = 256;

/// A unit of write work. Takes `&Connection` — `rusqlite::Transaction`
/// derefs to `Connection` but does NOT implement `DerefMut`, so the
/// writer cannot hand a `&mut Connection` to the job. Every rusqlite
/// write API (`execute`, `prepare`, `query_row`) only needs `&Connection`.
pub(crate) type WriteJob =
    Box<dyn FnOnce(&Connection) -> Result<(), FreeBibleError> + Send + 'static>;

struct Job {
    id: u64,
    label: &'static str,
    cancel: Arc<AtomicBool>,
    work: WriteJob,
    reply: SyncSender<Result<(), FreeBibleError>>,
}

#[derive(Clone, Debug)]
pub struct DbWriter {
    tx: SyncSender<Job>,
    next_id: Arc<AtomicU64>,
    depth: Arc<AtomicU64>,
    closing: Arc<AtomicBool>,
}

impl DbWriter {
    pub fn spawn(conn: Arc<Mutex<Connection>>) -> Self {
        let (tx, rx) = mpsc::sync_channel::<Job>(QUEUE_CAPACITY);
        let next_id = Arc::new(AtomicU64::new(0));
        let depth = Arc::new(AtomicU64::new(0));
        let closing = Arc::new(AtomicBool::new(false));

        let d = depth.clone();
        let c = closing.clone();
        thread::Builder::new()
            .name("xbible-dbw".into())
            .spawn(move || writer_loop(conn, rx, d, c))
            .expect("spawn xbible-dbw");

        Self {
            tx,
            next_id,
            depth,
            closing,
        }
    }

    pub fn depth(&self) -> u64 {
        self.depth.load(Ordering::Relaxed)
    }

    /// Signal the writer to stop after the current job.
    pub fn shutdown(&self) {
        self.closing.store(true, Ordering::Relaxed);
    }

    pub fn submit<F>(
        &self,
        label: &'static str,
        cancel: Arc<AtomicBool>,
        work: F,
    ) -> Result<(), FreeBibleError>
    where
        F: FnOnce(&Connection) -> Result<(), FreeBibleError> + Send + 'static,
    {
        debug_assert!(
            !is_main_thread(),
            "DbWriter::submit called on the main thread — move this to a task thread"
        );

        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled());
        }

        let (reply_tx, reply_rx) = mpsc::sync_channel::<Result<(), FreeBibleError>>(1);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);

        self.depth.fetch_add(1, Ordering::Relaxed);
        let job = Job {
            id,
            label,
            cancel,
            work: Box::new(work),
            reply: reply_tx,
        };

        if self.tx.send(job).is_err() {
            self.depth.fetch_sub(1, Ordering::Relaxed);
            return Err(FreeBibleError::Generic {
                error_message: "db writer thread is gone".into(),
            });
        }

        reply_rx.recv().unwrap_or_else(|_| {
            Err(FreeBibleError::Generic {
                error_message: "db writer dropped job without reply".into(),
            })
        })
    }
}

fn writer_loop(
    conn: Arc<Mutex<Connection>>,
    rx: Receiver<Job>,
    depth: Arc<AtomicU64>,
    closing: Arc<AtomicBool>,
) {
    loop {
        let job = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(job) => job,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if closing.load(Ordering::Relaxed) {
                    break;
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };

        depth.fetch_sub(1, Ordering::Relaxed);

        if job.cancel.load(Ordering::Relaxed) {
            let _ = job.reply.send(Err(cancelled()));
            continue;
        }

        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let mut guard = conn.lock().unwrap_or_else(|p| p.into_inner());
            let tx = guard.transaction().map_err(|e| FreeBibleError::Generic {
                error_message: format!("begin tx job#{} {}: {}", job.id, job.label, e),
            })?;

            // `&Transaction` coerces to `&Connection` via `Deref`.
            // The job runs inside the transaction; `commit` below
            // finalizes it. Dropping `tx` on the `?` path rolls back.
            (job.work)(&tx)?;

            tx.commit().map_err(|e| FreeBibleError::Generic {
                error_message: format!("commit job#{} {}: {}", job.id, job.label, e),
            })
        }));

        let result = match outcome {
            Ok(r) => r,
            Err(panic) => {
                let msg = panic_message(&panic);
                error!("db writer job#{} {} panicked: {}", job.id, job.label, msg);
                Err(FreeBibleError::Generic {
                    error_message: format!("db write panicked in {}: {}", job.label, msg),
                })
            }
        };

        let _ = job.reply.send(result);
    }

    info!("db writer thread exiting");
}

fn cancelled() -> FreeBibleError {
    FreeBibleError::Generic {
        error_message: "cancelled".into(),
    }
}

fn panic_message(p: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".into()
    }
}

fn is_main_thread() -> bool {
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "tvos",
        target_os = "visionos"
    ))]
    {
        unsafe extern "C" {
            fn pthread_main_np() -> i32;
        }
        unsafe { pthread_main_np() != 0 }
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "tvos",
        target_os = "visionos"
    )))]
    {
        thread::current().name() == Some("main")
    }
}
