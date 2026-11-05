# iksa

An agent-first browser for macOS with a Rust core. You give iksa a goal in plain language, and an agent works on it in a hidden, isolated browser context. Risky steps wait for your approval, CAPTCHAs are handed to you, and every action lands in a local, content-addressed audit log.

> **Status: early alpha.** The agent core (M1), goal planner (M2a), and daemon, schedules and suggestion inbox (M2b) work from the command line. The browser UI (M3) is planned. macOS on Apple Silicon only. Expect breaking changes.

## What it promises

- **Private.** No telemetry, no Google services. iksa never reads another browser's profile.
  - The only network traffic iksa itself starts is downloading its pinned Chromium build and calling the model provider you configured.
  - Form field values and named secrets never reach the model, and sites on your `never_send` list are never sent at all.
  - Every model request body is kept locally so you can see exactly what was shared.
- **Light.** The idle daemon measured 2.4 MiB with no Chromium child. When work needs a browser, one Chromium process tree serves everything: measured browser idle footprint is about 208 MiB, and 5 tabs plus 3 agents stay under 1.2 GB. The `iksa` binary is about 7 MB ([`docs/perf.md`](docs/perf.md)).
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

When a task pauses, `iksa list` shows why. Use `iksa approve <id>` for an approval or `iksa resume <id>` after solving a CAPTCHA or pausing a task yourself. The daemon picks it up, or you can use `iksa run` again.

For logins, store a named secret with `iksa secret set <name>` and refer to it in a goal as `{{secret:NAME}}`. The model only ever sees the placeholder.

Scripted tasks without a model also work: `iksa add task.json`, for example [`examples/hn.json`](examples/hn.json).

### Daemon, schedules, and inbox

`iksa daemon` stays running and picks up newly queued tasks. It starts Chromium
only when work is ready and closes the browser after 60 seconds without running
work. An idle daemon has no Chromium child. `iksa run` drains the current queue
and exits. One runner may own a data directory at a time.

With a model configured, add a recurring goal:

```sh
target/release/iksa schedule add 'every 30m' 'Check https://example.com/ and report its heading'
target/release/iksa schedule list
target/release/iksa inbox
```

Schedules use intervals such as `every 30m`, `every 2h`, or `every 1d` (minimum
five minutes), or five-field cron expressions evaluated in local time. Missed
fires collapse into one task, and an existing open task prevents another copy.
By default, scheduled goals and planner follow-ups enter the inbox as suggestions
and do not run until you `accept <id>`; `dismiss <id>` removes an unwanted idea.

`schedule add --auto <spec> <goal…>` or `schedule set <id> auto` queues scheduled
goals for execution directly. `schedule set <id> suggest` returns to inbox mode;
`schedule rm <id>` removes the schedule. Auto scheduling still applies the normal
action approval gates.

A standing grant lets **Write steps on that domain and its subdomains run
unattended until it expires**. For example, `grant shop.example --days 3` creates
a three-day grant; `--schedule <id>` limits it to one schedule. The default is
seven days and the allowed range is 1–30 days. Use `grant list` to inspect grants
and `grant rm <id>` to remove one. Each use checks expiry, schedule scope,
`never_send`, and the policy fingerprint. Changing `auto_max` or `never_send`
invalidates existing grants. **Financial steps always ask for approval** and
cannot use a grant. Audit records name each grant that allowed a step.

`pause <id>` stops a queued task or requests that a running task stop before its
next step; an action already in flight finishes first. `list` shows `paused` in
the waiting-on column. `resume <id>` queues it again. A resumed task fails safely
if restoring its page would require repeating a previous click. Goal tasks also
fail after three identical actions with unchanged observations, rather than
continuing a stuck loop.

`max_agents` defaults to **3** and accepts **1–8**. It bounds concurrent tasks,
with one additional reserved slot for human-started work while background tasks
are running. Set it in `config.json`; schedules and grants live in the ledger.

### Start at login on macOS

Save this as `~/Library/LaunchAgents/dev.iksa.daemon.plist`. Replace
`/absolute/path/to/iksa` with the absolute path to your built `target/release/iksa`
binary; LaunchAgents do not expand `~` in `ProgramArguments`.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.iksa.daemon</string>
  <key>ProgramArguments</key>
  <array>
    <string>/absolute/path/to/iksa</string>
    <string>daemon</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
</dict>
</plist>
```

Create `~/Library/LaunchAgents` if needed, then load the saved file:

```sh
launchctl bootstrap gui/$(id -u) "$HOME/Library/LaunchAgents/dev.iksa.daemon.plist"
```

The daemon uses the API key stored by `iksa key set` in your Keychain.
Do not put `ANTHROPIC_API_KEY` in the plist. To stop running it at login:

```sh
launchctl bootout gui/$(id -u) "$HOME/Library/LaunchAgents/dev.iksa.daemon.plist"
```

### Commands

| Command | What it does |
| --- | --- |
| `setup` | Install the pinned Chromium |
| `ask <goal…>` | Queue a natural-language goal (needs a model) |
| `add <task.json\|->` | Queue a scripted task |
| `run` | Run queued tasks until none are left |
| `daemon` | Stay running, dispatch due schedules and queued tasks, start the browser as needed |
| `schedule add [--auto] <spec> <goal…>` | Add a recurring goal; default is inbox mode |
| `schedule list` / `schedule set <id> auto\|suggest` / `schedule rm <id>` | Inspect, change or remove schedules |
| `inbox` / `accept <id>` / `dismiss <id>` | Inspect suggestions, queue one or dismiss it |
| `grant <domain> [--schedule <id>] [--days N]` | Allow unattended Write steps for a domain until expiry |
| `grant list` / `grant rm <id>` | Inspect or remove standing grants |
| `pause <id>` | Pause a queued task or stop a running task before its next step |
| `list` | Show tasks and their states |
| `approve <id>` / `resume <id>` | Approve a step or resume a challenge/manual pause |
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
| `max_agents` | Concurrent task limit (default 3, range 1–8), with an extra human-work slot while background tasks run |

## Development

```sh
cargo test -- --skip live_
cargo test --test acceptance_m2b -- --skip live_
CHROMIUM=$PWD/.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium \
  cargo test --release -- --ignored --test-threads=1 --skip live_
```

Browser tests are `#[ignore]` and need the pinned Chromium. Tests named `live_*` call the real model API and cost money, so they are excluded above. Plans for each milestone live in [`docs/superpowers/plans/`](docs/superpowers/plans/), and measurements live in [`docs/perf.md`](docs/perf.md).

## License

[MIT](LICENSE)
