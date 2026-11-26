# Security Policy

## Supported versions

`xbible_engine` is pre-1.0. Security fixes land on `main` and in the latest
`0.1.x` build; older builds are not maintained separately.

| Version | Supported |
|---|---|
| `0.1.x` (latest) | Yes |
| Anything older | No |

## Reporting a vulnerability

**Please do not open a public issue for a security problem.**

Use GitHub's private vulnerability reporting:

1. Go to [Security → Report a vulnerability](https://github.com/Edcorp844/xbible_engine/security/advisories/new).
2. Describe the issue, including the affected version or commit, a minimal
   reproduction, and the impact you believe it has.

If you cannot use that form, contact the maintainer
[@Edcorp844](https://github.com/Edcorp844) privately through GitHub and describe
the issue without including a working exploit.

### What to include

- The affected component (reader, search, store/install, audio engine, database
  layer, or binding generation).
- Steps to reproduce, ideally with the input that triggers it.
- Whether the input comes from a remote source (a provider response) or from
  data already on disk.
- Your assessment of impact: information disclosure, code execution, denial of
  service, or corrupted data.

## What to expect

- Acknowledgement of your report, normally within a few days.
- An initial assessment and a plan, or a clear explanation if the report is not
  considered a vulnerability.
- Credit in the advisory once a fix ships, unless you ask not to be named.

This project is maintained on a best-effort basis and does not currently run a
paid bounty programme.

## Scope

In scope:

- Parsing of untrusted input: provider API responses, `.xba` audio modules,
  module archives and bundled datasets.
- Memory safety and panics reachable from that input, including across the
  UniFFI boundary.
- SQL handling in `src/db/` and installation paths in `sources/`.
- The build and binding-generation scripts, insofar as they affect what gets
  shipped.

Out of scope:

- Vulnerabilities in the content itself (a mistranslated verse is not a security
  issue).
- Issues that require an attacker to already control the device or the database
  file.
- Denial of service from simply installing very large content.
- Vulnerabilities in upstream dependencies that have no exploitable path here —
  report those upstream, but do tell us if you think we are affected.

## Handling of data

The engine stores library content in a local SQLite database whose path the host
application chooses. It does not collect telemetry, and it does not transmit
user data anywhere; network requests are limited to fetching content from the
configured provider.
