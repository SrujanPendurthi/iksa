# Developer & Architecture Guidelines

Rust package with a library and CLI binary `iksa`. `cdp.rs` is our own CDP client over `--remote-debugging-pipe` (Spike 2 decision). One headful ungoogled-chromium; each task gets its own browser context and a `hidden` target with a 1280×800 viewport. Pure modules (CAS, snapshot, config/gate/action, challenge, ledger, audit) sit under `engine.rs` and a std-args CLI. No LLM in M1.

Module map (implemented through `engine`; `setup` and the std-args CLI remain planned; library modules live in `src/lib.rs`): `cdp` — pipe CDP transport and browser lifecycle; `cas` — immutable BLAKE3 blob storage; `snapshot` — content-defined DOM chunks; `config` — settings and search providers; `gate` — risk decisions; `action` — scripted actions and secret redaction; `challenge` — challenge detection; `ledger` — SQLite task state; `audit` — records, reports, retention; `engine` — sequential isolated CDP task execution with risk gates, snapshots, challenge pauses and bounded context cleanup; `setup` — pinned Chromium installation; `main` — std-args CLI.

Unit tests live in-module: `cargo test`.
Chromium tests live in `tests/` with `#[ignore]`:

```sh
CHROMIUM=$PWD/.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium cargo test --release -- --ignored --test-threads=1
```

Ignored tests must be run before a task is done, and must pass.

Codex implements; Claude plans and reviews.
