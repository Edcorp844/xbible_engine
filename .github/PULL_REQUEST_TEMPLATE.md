## Summary

<!-- What does this change and why? One paragraph is usually enough. -->

## Related issue

<!-- e.g. Closes #12, or "no issue — small fix" -->

## Type of change

- [ ] Bug fix
- [ ] New feature
- [ ] Refactor (no behaviour change)
- [ ] Documentation
- [ ] Build / CI / tooling

## How this was verified

<!-- Paste the commands you ran and their outcome, e.g. cargo test, cargo clippy. -->

- [ ] `cargo test` passes
- [ ] `cargo clippy --all-targets` is clean
- [ ] `cargo fmt --check` passes
- [ ] Examples still compile (if public API changed)

## Checklist

- [ ] No new `unwrap`/`expect`/`panic` on the exported path
- [ ] Schema change is a migration in `src/db/migrations.rs` (if applicable)
- [ ] `db/` does not depend on provider wire types (if applicable)
- [ ] New or updated tests cover the change
- [ ] Public API changes are reflected in `ARCHITECTURE.md` or `README.md`

## Screenshots / notes

<!-- Optional: UI impact, manual test notes, anything a reviewer should know. -->
