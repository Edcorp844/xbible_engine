# Contributing to xbible_engine

Thanks for taking the time to contribute. This document covers the practical
parts: getting a working build, the standards the code is held to, and how
changes get reviewed.

By participating you agree to follow our [Code of Conduct](CODE_OF_CONDUCT.md).

## Before you start

- For anything larger than a small fix, **open an issue first** (or comment on an
  existing one) so the approach can be agreed before you invest time. The issue
  forms in `.github/ISSUE_TEMPLATE/` cover bug reports, tasks and sprint
  planning.
- Security problems must **not** be filed as public issues — see
  [SECURITY.md](SECURITY.md).

## Development setup

Requirements: Rust 1.85+ (the crate uses edition 2024) and a C toolchain for
`rusqlite`'s bundled SQLite.

```bash
git clone https://github.com/Edcorp844/xbible_engine.git
cd xbible_engine

./setup.sh          # optional: add cross-compilation targets
cargo build
cargo test
cargo clippy --all-targets
cargo fmt --check
```

Native-only work (reader, store, database) needs nothing beyond `cargo test`.
Cross-platform artifacts come from `./build.sh`, which also generates the
language bindings; it expects the corresponding target toolchains to be
installed.

## Code standards

- **No panics on the exported path.** The crate denies `unwrap_used`,
  `expect_used`, `panic`, `indexing_slicing`, `unimplemented`, `todo` and `exit`
  at the clippy level. Return `Result<_, XBibleError>` instead.
- **Respect the layering** described in [ARCHITECTURE.md](ARCHITECTURE.md):
  `lib.rs` → `features/` + `engines/` → `sources/` → `db/`. In particular `db/`
  must not learn about provider wire formats; normalize into `sources::models`
  first.
- **Schema changes go through `db/migrations.rs`.** Never create or alter tables
  outside a migration, and never commit a database file.
- **The FFI boundary is explicit.** Only items marked `uniffi` are public API;
  keep implementation modules internal and use owned, FFI-friendly types
  (`String`, `Vec`, `Option`, `bool`, `i32`, `i64`).
- **Clippy is part of the build.** Pedantic and nursery are denied, so
  `cargo clippy --all-targets` should be clean before you open a PR.
- Run `cargo fmt` before committing.

## Tests

- Add or update tests in `tests/` for behaviour changes — `db_smoke`,
  `navigation_tree_test` and `store_smoke` show the existing patterns.
- `examples/` are runnable smoke tests against the public API; keep them
  compiling when you change a signature.
- Bug fixes should come with a test that fails before the fix and passes after.

## Commits and pull requests

- Keep commits focused and give them a message that explains *why* the change is
  needed, not just what changed.
- Open the PR against `main` and fill in the pull request template.
- Describe how you verified the change (`cargo test`, `cargo clippy`, manual
  runs) — reviewers rely on that more than on the diff alone.
- CI is not yet automated, so please paste the relevant command output in the PR
  if it is short.
- Releases go through a Production Release issue that the maintainer approves;
  merging to `main` does not itself publish anything.

## Reviewing

Reviews look for: correctness, adherence to the layering and panic-free rules, a
test that actually covers the change, and a migration where the schema moved.
Expect follow-up questions on any of those rather than a silent merge.

## License

By contributing you agree that your contributions are licensed under the
[GNU General Public License v3.0](LICENSE), the same license as the project.
