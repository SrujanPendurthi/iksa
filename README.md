# iksa

An agent-first browser for macOS with a Rust core. You give iksa a goal in plain language, and an agent works on it in a hidden, isolated browser context. Risky steps wait for your approval, CAPTCHAs are handed to you, and every action lands in a local, content-addressed audit log.

> **Status: early alpha.** The agent core (M1) and the goal planner (M2a) work from the command line. The background daemon, schedules and suggestion inbox (M2b) and the browser UI (M3) are planned. macOS on Apple Silicon only. Expect breaking changes.

## What it promises

- **Private.** No telemetry, no Google services. iksa never reads another browser's profile.
  - The only network traffic iksa itself starts is downloading its pinned Chromium build and calling the model provider you configured.
  - Form field values and named secrets never reach the model, and sites on your `never_send` list are never sent at all.
  - Every model request body is kept locally so you can see exactly what was shared.
- **Light.** One Chromium process tree for everything. Measured idle footprint is about 208 MiB, and 5 tabs plus 3 agents stay under 1.2 GB. The `iksa` binary is about 7 MB ([`docs/perf.md`](docs/perf.md)).
- **Gated.** Clicks and typing pause for approval by default. Financial actions always require you, and no setting changes that.
- **Auditable.** `iksa report <id>` shows the goal, each step, what the model was told and why it acted. DOM snapshots are deduplicated with BLAKE3 + zstd, so a typical task stores well under 500 KB.

## How it works

iksa drives a pinned [ungoogled-chromium](https://github.com/ungoogled-software/ungoogled-chromium) build through its own Chrome DevTools Protocol client over a pipe, with no debugging port. Each task gets its own browser context and a hidden page.

The model sees a pruned list of page elements with numbered refs, never raw HTML, and picks one action per step: navigate, search, click, type, extract or done. iksa turns each choice into an ordinary audited action that goes through the same risk gates as a hand-written task. Task state lives in SQLite; snapshots live in a content-addressed store.

See [`PRD.md`](PRD.md) for the full design and milestones.

## Requirements

- macOS on Apple Silicon (arm64)
- Rust 1.97 or newer
- An Anthropic API key for goal tasks (scripted tasks need no model)

## Install

```sh
cargo build --release
target/release/iksa setup    # downloads and verifies the pinned Chromium into ~/.iksa
target/release/iksa list
```

Data lives in `~/.iksa` (owner-only). Set `IKSA_HOME` to use another directory.

## Use

Configure a model by merging this into `~/.iksa/config.json`:

```json
{ "model": { "provider": "anthropic", "name": "claude-opus-5-5" } }
```

`claude-sonnet-5-5` and `claude-haiku-5-5` cost less. Then store your API key in the Keychain; it prompts, so the key never appears in your shell history:

```sh
target/release/iksa key set
```

Queue a goal and run it:

```sh
target/release/iksa ask 'Open https://example.com/ and report the page heading'
target/release/iksa run
target/release/iksa report 1
```

When a task pauses, `iksa list` shows why. Use `iksa approve <id>` for an approval or `iksa resume <id>` after solving a CAPTCHA, then `iksa run` again.

For logins, store a named secret with `iksa secret set <name>` and refer to it in a goal as `{{secret:NAME}}`. The model only ever sees the placeholder.

Scripted tasks without a model also work: `iksa add task.json`, for example [`examples/hn.json`](examples/hn.json).

### Commands

| Command | What it does |
| --- | --- |
| `setup` | Install the pinned Chromium |
| `ask <goal…>` | Queue a natural-language goal (needs a model) |
| `add <task.json\|->` | Queue a scripted task |
| `run` | Run queued tasks until none are left |
| `list` | Show tasks and their states |
| `approve <id>` / `resume <id>` | Continue a task paused for approval or a challenge |
| `report <id>` | Markdown audit report for a task |
| `key set` / `secret set <name>` | Store the API key or a named secret in the Keychain |
| `gc` | Remove tasks and blobs older than `retention_days` |

### Configuration

`~/.iksa/config.json` is created with defaults on first run:

| Field | Meaning |
| --- | --- |
| `search`, `providers` | Search provider (DuckDuckGo, Kagi, Brave, SearXNG, or your own URL template with `{q}`) |
| `auto_max` | Highest risk that runs without approval: `read` (default) or `write`; `financial` always asks |
| `retention_days` | How long finished tasks are kept (default 30) |
| `model` | `provider`, `name`, and optional `endpoint`, `effort` (default `low`), `max_steps` (default 25), `fallbacks` (default `true`) |
| `never_send` | Domains whose pages are never sent to a model |

## Development

```sh
cargo test
CHROMIUM=$PWD/.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium \
  cargo test --release -- --ignored --test-threads=1 --skip live_
```

Browser tests are `#[ignore]` and need the pinned Chromium. Tests named `live_*` call the real model API and cost money, so they are excluded above. Plans for each milestone live in [`docs/superpowers/plans/`](docs/superpowers/plans/), and measurements live in [`docs/perf.md`](docs/perf.md).

## License

[MIT](LICENSE)
