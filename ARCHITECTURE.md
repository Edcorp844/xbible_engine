# Project Architecture

`xbible_engine` is a Rust crate that serves Bible content to Kotlin (Android) and
Swift (iOS/macOS) clients over a single UniFFI boundary. It owns the on-device
SQLite library, fetches content from external providers, and exposes reading,
search, store and timeline features through one entry object.

## Runtime Boundary

`src/lib.rs` is the only place UniFFI scaffolding is declared
(`uniffi::setup_scaffolding!()`). The crate builds as `lib`, `staticlib` and
`cdylib`, and `src/bin/uniffi-bindgen.rs` is the developer CLI that generates
language bindings — it is not part of the runtime API.

The crate root exports two entry points:

- `XBibleEngine` (`uniffi::Object`) — the single entry point. Constructing it via
  `XBibleEngine::new()` initializes platform logging as a side effect; there is no
  module initializer to rely on.
- `init_logging()` — an exported free function that re-runs the same
  initialization for embedders that want it explicitly.

`XBibleError` (`uniffi::Error`, `error.rs`) is the error type of every fallible
exported method. Internally it wraps database and provider failures as
`XBibleError::Generic { error_message }`, so details are preserved as text rather
than as typed variants across the boundary.

Platform logging is selected at compile time: `android_logger` on Android,
`env_logger` elsewhere, with `oslog` as the Apple fallback. On iOS, `libbz2` is
built with `BZ_NO_STDIO`, so `bz_internal_error` is exported as a C hook that
logs and aborts instead of touching stdio.

`XBibleEngine` owns the shared state and exposes feature handles, each returned
as an `Arc` of a `uniffi::Object`:

| Handle | Returns | Owns |
|---|---|---|
| `reader()` | `XBibleReader` | Read path over the shared connection, search |
| `store()` | `Store` | Catalog browsing, installation, task tracking |
| `audio_engine()` | `AudioEngine` | Audio module parsing and local cache |
| `timeline()` | `TimelineData` | Static Bible-history periods and events |

## Source Layout

```text
src/
  lib.rs                    UniFFI scaffolding, XBibleEngine, logging, C hooks
  bin/uniffi-bindgen.rs     Binding generation CLI (developer tool)
  error.rs                  XBibleError — the cross-boundary error type
  runtime.rs                Shared multi-threaded Tokio runtime (OnceLock)
  db/                       SQLite infrastructure shared by all features
    connection.rs           Db<Closed>/Db<Open>: open + pragmas + migrations
    connection_state.rs     Sealed typestate markers (Closed, Open)
    writer.rs               DbWriter: single writer thread, bounded job queue
    migrations.rs           Schema versioning (rusqlite_migration)
    catalog.rs              Persisted translation catalog
    install.rs              Generic persistence of installed content
    error.rs                DbError
  features/                 User-facing features behind XBibleEngine handles
    reader/                 XBibleReader; search.rs holds search modes/filters
    store/                  Store: catalog, categories, install tasks
    timeline/               TimelineData + embedded datasets
  engines/
    audio_engine/           AudioEngine, XBA store client, artwork/color utils
  sources/                  Provider integrations and normalized models
    mod.rs                  BibleSource / SourceInstaller traits, SourceRegistry
    models/                 Generic* types handed to db/ and features/
    helloao/                HelloAO provider: client, catalog, entries, values
```

`engines/` now contains only the audio engine. The earlier per-feature engines
(`module_engine`, `osis_translation_engine`, `freebible_engine`, and the
`xbible_engine` facade directory) were retired: reading, store and timeline moved
to `features/`, provider code moved to `sources/`, and the public facade moved to
the crate root.

## Layering

Dependencies point inward, one direction only:

```text
lib.rs  →  features/  engines/  →  sources/  →  db/  +  runtime  +  error
```

- `db/` is infrastructure. It consumes normalized `Generic*` types and must not
  depend on provider HTTP wire formats — that is what keeps persistence uniform
  across sources.
- `sources/` owns everything provider-specific: endpoints, auth, raw JSON
  parsing, pagination and quirks. It normalizes every payload into
  `sources::models` before anything else sees it.
- `features/` implements user-facing behaviour on top of those models and the
  shared connection.
- `lib.rs` composes the pieces and defines the FFI boundary.

## Database Model

`Db<S>` uses a sealed typestate (`connection_state.rs`): `Db<Closed>` only offers
`open`, which consumes itself and returns `Db<Open>`. Query and write methods
exist only on `Open`, so a stale handle cannot be used after a close. Downgrading
is not implicit — the engine drops the `Db<Open>` and starts over.

`Db::<Closed>::open(path)` sets `journal_mode=WAL`, `synchronous=NORMAL`,
`busy_timeout=5000`, `foreign_keys=ON`, then runs migrations. Schema changes
always go through `db/migrations.rs`; a database file is never committed, since
it is rebuilt from migrations on a fresh clone.

Writes do not go through the shared connection directly. `DbWriter` owns a single
writer thread fed by a bounded (256) queue of closures; each job carries a
cancellation flag and a reply channel. This serializes all writes, keeps SQLite's
single-writer rule, and lets in-flight work be cancelled during a database swap.
Reads use the shared `Arc<Mutex<Connection>>` directly.

## Concurrency

- One shared Tokio multi-threaded runtime (`runtime::get_shared_runtime()`)
  services all network and background work.
- One SQLite connection behind `Arc<Mutex<Connection>>`; reads share it, writes are
  funneled through `DbWriter`.
- Feature objects are `Arc`-shared and use `RwLock`/`Mutex` internally, so UniFFI
  can call them from any thread.
- `Store` tracks installation work as tasks with ids, progress and cancellation;
  `cancel_all_tasks()` is what makes a database swap safe.

## Changing the Database Path

`XBibleEngine::set_db_path` is idempotent and safe to call across app lifecycle
or file moves. Order matters:

1. Cancel running store tasks that hold clones of the old writer.
2. Shut down and drop the old `DbWriter`.
3. Drop the old `Db<Open>` so the file handle closes.
4. Open the database at the new path (runs migrations).
5. Spawn a fresh `DbWriter` and attach the shared connection to `reader` and
   `store`.

## Adding a Source

1. Add a directory under `src/sources/` for the provider (client, catalog, raw
   deserialization) and keep wire types private to it.
2. Normalize every payload into the `Generic*` types in `src/sources/models/` —
   `db/` and `features/` only ever see those.
3. Implement `BibleSource` (and `SourceInstaller` if it installs content), then
   register it with the `SourceRegistry` used by `Store`.
4. Report progress through `InstallProgressSink` using the `InstallPhase` steps
   (catalog, download, normalize) so `Store` tasks show meaningful state.

## Adding a Feature

1. Create a directory under `src/features/` (engine and implementation modules).
2. Register it in `src/features/mod.rs`.
3. Add a field and a handle method on `XBibleEngine` when it should be reachable
   from Kotlin/Swift, and wire it in both constructors (`new` and `Default`).
4. Attach the shared connection in `set_db_path` if it needs the database.
5. Annotate only the intended boundary with `uniffi` derives/exports. UniFFI types
   must use owned, FFI-friendly types (`String`, `Vec`, `Option`, `bool`, `i32`,
   `i64`); nested collections stay `Vec` rather than maps.

## Tests and Examples

`tests/` holds integration coverage (`db_smoke`, `navigation_tree_test`,
`store_smoke`). `examples/` are runnable consumers that double as manual smoke
tests: `reader_example`, `store_example`, `timeline_example`, `entity_example`.
Work that outlived its feature lives under `later/` and is not part of the build.

## Invariants

- No panics on the exported path: clippy denies `unwrap_used`, `expect_used`,
  `panic`, `indexing_slicing`, `unimplemented`, `todo`, and `exit`; errors
  propagate as `Result<_, XBibleError>`.
- Only `Db<Open>` can query or write.
- Every schema change is a migration; no database file is tracked in git.
- `db/` never depends on provider wire types.
- Only `lib.rs` declares UniFFI scaffolding; only intended items are exported.
