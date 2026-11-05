# Developer & Architecture Guidelines

Rust package with a library and CLI binary `iksa`. `cdp.rs` is our own CDP client over `--remote-debugging-pipe` (Spike 2 decision). One headful ungoogled-chromium; each task gets its own browser context and a `hidden` target with a 1280×800 viewport. Pure modules (CAS, snapshot, config/gate/action, challenge, ledger, audit) sit under `engine.rs` and a std-args CLI. Since M2a, `observe`/`llm`/`planner` drive goal tasks with a configured Claude model. M2b adds the daemon, local schedules, suggestion inbox, standing grants, pause/resume and stuck detection.

Module map (library modules live in `src/lib.rs`; the CLI lives in `src/main.rs`): `cdp` — pipe CDP transport and browser lifecycle; `cas` — immutable BLAKE3 blob storage; `snapshot` — content-defined DOM chunks; `config` — settings and search providers; `gate` — risk decisions; `action` — scripted actions and secret redaction; `challenge` — challenge detection; `ledger` — SQLite task state; `audit` — records, reports, retention; `engine` — isolated CDP task execution with risk gates, snapshots, challenge pauses and bounded context cleanup; `keychain` — macOS Keychain access for the API key and named secrets (fake store via `IKSA_MOCK_KEYCHAIN`); `llm` — Anthropic Messages client and request bodies; `observe` — pruned page observation with numbered refs; `planner` — goal planner system prompt, tool schemas and model-tool-call to action mapping; `schedule` — interval/cron specs and local next-fire time; `daemon` — dispatcher for schedules, inbox, bounded workers and lazy browser; `setup` — pinned Chromium installation; `main` — std-args CLI.

Unit tests live in-module: `cargo test`.
Chromium tests live in `tests/` with `#[ignore]`:

```sh
CHROMIUM=$PWD/.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium cargo test --release -- --ignored --test-threads=1 --skip live_
```

`live_*` tests call the real model API and spend money; run them only with the user's explicit go-ahead.

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

Run validation without live model tests:

```sh
cargo test -- --skip live_
CHROMIUM=$PWD/.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium cargo test --release -- --ignored --test-threads=1 --skip live_
```

Live evaluations require separate approval for API spend.
