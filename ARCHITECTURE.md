# Project Architecture

## Public Boundary

`XBibleEngine` is the library's top-level object for UniFFI consumers. It is re-exported at the crate root and at `engines::XBibleEngine`; feature handles are reached through its `audio()` and `freebible()` methods. UniFFI scaffolding is declared once in `src/lib.rs`; only types and functions explicitly annotated for UniFFI are part of the foreign-language API.

UniFFI generates bindings in a crate-level module; it does not create nested language namespaces under a class. `XBibleEngine` is therefore the entry-point object, while returned feature handles remain distinct foreign-language object types.

Constructing `XBibleEngine` initializes platform logging before feature operations begin. UniFFI has no module-import initializer, so setup belongs in the object's constructor. The root-level `init_logging` function remains a Rust convenience for existing Rust callers; it is not a separate UniFFI function.

## Source Layout

```text
src/
  lib.rs                    UniFFI scaffolding and library-level FFI hooks
  bin/uniffi-bindgen.rs     Developer CLI for generating language bindings
  engines/
    xbible_engine/          Public facade and its initialization
    freebible_engine/       FreeBible feature: client, reader, store, catalog
    audio_engine/           Audio feature: engine, local store, service, utilities
  db/                       Shared SQLite connection, state, errors, migrations
  data/                     Shared data models and datasets
  runtime.rs                Shared Tokio runtime
```

Each `*_engine` directory owns one feature. Feature-specific models, network clients, persistence adapters, and utilities belong beside that feature. Shared modules should contain only infrastructure used across features. In particular, FreeBible catalog mappings live with FreeBible rather than in shared `db/` code.

`src/bin/uniffi-bindgen.rs` is correctly located under `src/bin/`: Cargo builds it as a command-line executable, separate from the library's runtime API. `uniffi.toml` configures binding generation, and generated bindings/framework artifacts are outputs rather than Rust source modules.

## Adding a Feature

1. Add a dedicated directory under `src/engines/` for the feature's engine and implementation modules.
2. Register the feature module in `src/engines/mod.rs`.
3. Add a handle or methods to `XBibleEngine` when the feature should be reachable by foreign-language consumers.
4. Annotate only the intended boundary with UniFFI derives/exports; keep implementation modules internal where possible.
5. Keep shared database/runtime/data code feature-neutral, and document any feature-owned schema additions alongside their owner.