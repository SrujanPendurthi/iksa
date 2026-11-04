# Developer & Architecture Guidelines

Rust package with a library and CLI binary `iksa`. `cdp.rs` is our own CDP client over `--remote-debugging-pipe` (Spike 2 decision). One headful ungoogled-chromium; each task gets its own browser context and a `hidden` target with a 1280×800 viewport. Pure modules (CAS, snapshot, config/gate/action, challenge, ledger, audit) sit under `engine.rs` and a std-args CLI. No LLM in M1.

Module map (library modules live in `src/lib.rs`; the CLI lives in `src/main.rs`): `cdp` — pipe CDP transport and browser lifecycle; `cas` — immutable BLAKE3 blob storage; `snapshot` — content-defined DOM chunks; `config` — settings and search providers; `gate` — risk decisions; `action` — scripted actions and secret redaction; `challenge` — challenge detection; `ledger` — SQLite task state; `audit` — records, reports, retention; `engine` — sequential isolated CDP task execution with risk gates, snapshots, challenge pauses and bounded context cleanup; `setup` — pinned Chromium installation; `main` — std-args CLI.

Unit tests live in-module: `cargo test`.
Chromium tests live in `tests/` with `#[ignore]`:

```sh
CHROMIUM=$PWD/.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium cargo test --release -- --ignored --test-threads=1
```

Ignored tests must be run before a task is done, and must pass.

Codex implements; Claude plans and reviews.

## README

Build and install the pinned Chromium, then initialize the data directory:

```sh
cargo build --release
target/release/iksa setup
target/release/iksa list
```

Data defaults to `~/.iksa`; set `IKSA_HOME` to use another directory. No model is configured by default. Merge these fields into the existing `config.json` there, keeping its search and risk settings:

```json
{
  "model": {
    "provider": "anthropic",
    "name": "claude-opus-5-5"
  },
  "never_send": ["bank.example"]
}
```

The model endpoint defaults to `https://api.anthropic.com`, effort to `low`, `max_steps` to 25, and `fallbacks` to true. These are optional fields inside `model`. API keys belong in the Keychain, outside `config.json`. Set the key interactively:

```sh
target/release/iksa key set
```

`ANTHROPIC_API_KEY` overrides the Keychain key when set. Queue a goal, inspect the queue, then execute it:

```sh
target/release/iksa ask 'Open https://example.com/ and report the page heading'
target/release/iksa list
target/release/iksa run
```

`ask` joins the remaining arguments into one goal and prints its task ID. It requires a configured model and only queues the task. `list` marks goal tasks with `[goal]`; scripted JSON tasks still use `add <task.json|->`. Empty goals are usage errors.

By default, clicks and typing pause for approval. Use `approve <id>` followed by `run` to continue. Financial actions always require approval. A CAPTCHA pause uses `resume <id>` after the challenge is resolved. Domains on `never_send` pause before a model request; change that list only when you intend to allow sharing their page data.

Store a named secret interactively with `secret set <name>`, then refer to it in a goal as `{{secret:NAME}}`:

```sh
target/release/iksa secret set demo
```

Names contain letters, digits, `_` or `-`. Secret values are read from the Keychain when used; stored actions keep the placeholder. Both key and secret setters prompt for the value, so it is absent from command arguments. Tests use the fake Keychain only when both `IKSA_MOCK_KEYCHAIN=1` and `IKSA_KEYCHAIN_FAKE=<directory>` are set.

Use `report <id>` for the goal, step outcomes, model text summaries, and model-request hashes. Each exact outgoing model request body is retained in the local CAS for audit. Model calls carry the pruned page observation and a compact step log; known secrets and form values are redacted.

Run validation without live model tests:

```sh
cargo test -- --skip live_
CHROMIUM=$PWD/.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium cargo test --release -- --ignored --test-threads=1 --skip live_
```

Live evaluations require separate approval for API spend.
