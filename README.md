# xbible_engine

Offline-first Bible engine in Rust, with generated bindings for Swift, Kotlin and
other languages through [UniFFI](https://mozilla.github.io/uniffi-rs/).

`xbible_engine` downloads Bible translations, commentaries and study datasets
from online providers, stores them in a local SQLite library, and exposes
reading, search, store and timeline features to mobile and desktop apps through
a single FFI boundary.

## Features

- **Reading** — installed translations, canon sections, books, chapters, verses,
  footnotes, formatting spans and right-to-left text.
- **Search** — full-text search with selectable modes and filters, plus progress
  reporting and query classification.
- **Store** — catalog browsing by category and language, with install and
  uninstall running as cancellable background tasks.
- **Study content** — commentaries and entity datasets (people, places, events,
  groups) linked to their verse references.
- **Timeline** — an embedded Bible-history timeline of periods and events.
- **Audio** — `.xba` audio module parsing with a local cache.
- **Provider-agnostic persistence** — the [HelloAO](https://bible.helloao.org)
  provider is normalized into shared `Generic*` models, so storing and reading
  content does not depend on any provider's wire format.

## Architecture

The module layout, FFI boundary, database model and layering rules are documented
in [ARCHITECTURE.md](ARCHITECTURE.md).

## Requirements

- Rust 1.85 or newer (the crate uses edition 2024).
- Platform targets for cross-compilation: run `./setup.sh` to add the rustup
  targets for macOS, Linux, Android and Windows.
- Binding generation: `./build.sh` prompts for target platforms and output
  languages, then builds the static library and generated bindings.

## Quick start

```bash
cargo build            # build the library
cargo test             # run the integration tests in tests/
cargo run --example reader_example
```

## Usage

```rust
use xbible_engine::XBibleEngine;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = XBibleEngine::new();
    engine.set_db_path("/tmp/xbible.db".to_string())?;

    // Browse the catalog and start an install task.
    let store = engine.store();
    let items = store.items_by_category("featured".to_string(), None);
    if let Some(item) = items.into_iter().next() {
        let task_id = store.install(item)?;
        println!("install started: {task_id}");
    }

    // Read from whatever is installed.
    let reader = engine.reader();
    for translation in reader.get_installed_translations() {
        println!("{} ({})", translation.name, translation.short_name);
    }

    Ok(())
}
```

`engine.reader()`, `engine.store()`, `engine.audio_engine()` and
`engine.timeline()` return handles you can hold on their own. Installation
progress and cancellation are polled through `store.poll_task(id)` and
`store.cancel_task(id)`.

## Data and storage

Content lives in a single SQLite database whose path is chosen by the host app
via `set_db_path`; the engine sets WAL mode and applies schema migrations on
open. The database is runtime state and is never committed to this repository —
a fresh clone creates and migrates its own database.

The database path can be changed at any time; the engine cancels in-flight store
tasks, closes the old handle and reattaches its features to the new one.

## Platform support

The crate builds as `lib`, `staticlib` and `cdylib`. `./build.sh` assembles
per-platform artifacts for macOS (Intel and Apple Silicon), iOS (device and
simulator), Android (arm64 and x86_64), Linux and Windows, and can generate
Swift, Kotlin, C#, Java, C, C++, Python and Ruby bindings.

## Testing

Integration tests live in `tests/` (`db_smoke`, `navigation_tree_test`,
`store_smoke`). The programs in `examples/` exercise the public API end to end
and double as manual smoke tests.

## Project tracking

Work is tracked in GitHub issues and sprints through Delivery OS; open a
Production Release issue to request a release approval.

## Contributing

Contributions are welcome — please read [CONTRIBUTING.md](CONTRIBUTING.md) and
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) first. Report security issues privately
as described in [SECURITY.md](SECURITY.md).

## License

Licensed under the [GNU General Public License v3.0](LICENSE) (`GPL-3.0-only`).
