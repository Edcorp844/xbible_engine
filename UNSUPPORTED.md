## ⚠️ Engine Migration Notice

This branch previously used **SWORD** (the CrossWire Bible Society library) and
**SWORD modules** for Bible text storage and retrieval.

The engine has since been **migrated off SWORD** and now uses the **FreeBible API**
(`freebible_engine`) instead.

### What changed

| Area              | Before                              | After                                   |
|-------------------|-------------------------------------|-----------------------------------------|
| Text source       | SWORD library (`libsword`) + SWORD modules | FreeBible API (`freebible_engine`) |
| Module storage    | Local SWORD module directories      | Remote FreeBible API (e.g. `bible.helloao.org`) |
| Bindings          | `bindgen` over `flatapi.h` (SWORD)  | `freebible_engine::engines::freebible_engine::store_api` |
| Build dependencies| CMake build of SWORD, vendored curl for iOS/Android | Rust-native HTTP client + JSON API |

### Status of SWORD-related features

- **No longer supported.** The SWORD library, SWORD module handling, and the
  CMake/build-script plumbing for `libsword` are deprecated on this branch.
- Install/retrieval paths that assumed local SWORD modules (e.g.
  `install adam-clarke`) now go through the FreeBible API instead.
- Any behavior that depended on SWORD-specific module formats is **not**
  guaranteed to work.

### If you are on an older revision

Older revisions of this branch still contain the SWORD-based engine. Those
revisions are **unsupported** and kept for historical reference only.

For current development, use the FreeBible API path described above.