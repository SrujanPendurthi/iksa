# iksa — M1 Agent Core v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Supersedes:** `2026-10-28-agentic-browser-m1-agent-core.md` (all tasks) and `2026-10-28-spike-2-unblock-m1.md` Tasks 4–6. Those files stay as history; do not execute them.

**Goal:** A CLI agent core: queue scripted tasks, run them in hidden isolated Chromium tabs behind risk gates, pause on CAPTCHA or approval, and emit a private, deduplicated audit log plus Markdown report — with memory and egress checked by tests.

**Architecture:** Single Rust binary crate `iksa`. `cdp.rs` is our own CDP client over `--remote-debugging-pipe` (Spike 2 decision). One headful ungoogled-chromium; each task gets its own browser context and a `hidden` target with a 1280×800 viewport. Pure modules (CAS, snapshot, config/gate/action, challenge, ledger, audit) sit under `engine.rs` and a std-args CLI. No LLM in M1.

**Tech Stack:** Rust 1.97 (edition 2024), tokio (`rt-multi-thread`, `macros`, `sync`, `time`, `process`), serde/serde_json, anyhow, thiserror 2, libc, blake3 1.8, zstd, rusqlite 0.41 (`bundled`).

**Spec:** `PRD.md` (now `gemini-code-1793212240267.md`; Task 0 renames). Evidence: `docs/spikes.md`.

## Facts from the spikes (`docs/spikes.md`) this plan relies on

- Pin: ungoogled-chromium `156.0.8078.47-1.1` arm64, URL `https://github.com/ungoogled-software/ungoogled-chromium-macos/releases/download/156.0.8078.47-1.1/ungoogled-chromium_156.0.8078.47-1.1_arm64-macos.dmg`, SHA-256 `5439d292078e260928d202145d0a20f19f227424888e84beb46973c16d40a186`. Dev copy: `.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium`.
- Hidden targets (`Target.createTarget { hidden: true, browserContextId }`) have no window and report CDP type `other`. Attach with `flatten: true`. With `Emulation.setDeviceMetricsOverride` 1280×800 they screencast (60 frames/6 s once acked) and accept `Input.dispatchMouseEvent` + `Input.insertText`.
- Memory, summed phys_footprint: baseline 398 MB idle; with `--disable-features=PreloadTopChromeWebUI,WebUIOmniboxPopup,WebUIOmniboxAimPopup` 223 MB idle, 366–385 MB with one hidden Wikipedia agent.
- Egress: idle 120 s with the privacy args gave 0 hosts in the netlog. No live positive control yet; Task 8 adds one.
- Turnstile: nowsecure.nl embeds a widget on a page that loads fine (200). Turnstile sets its token as the input's `.value` *property*, so `outerHTML` never shows it; the token must be read via JS.
- Native messaging host manifests live in `<user-data-dir>/NativeMessagingHosts/` (needed in M3, not here).
- Launching Chromium from Codex triggered a Keychain popup; `--use-mock-keychain` is only for tests.

## Global Constraints

- Binary < 60 MB. Step overhead < 25 ms (gate + snapshot + CAS write, excluding Chromium). Storage < 500 KB per completed task.
- Memory: summed phys_footprint over the process tree, median of 3: ≤ 300 MB idle, ≤ 1.2 GB for 5 tabs + 3 agents.
- Privacy: idle browser makes zero non-loopback connections; no TCP debugging port; no telemetry.
- BLAKE3 hashes; immutable blobs. Risk 2 (financial) always needs sign-off.
- Lifecycle `Queued -> Executing -> AwaitingHandoff -> Committed` (+ `Failed`).
- Data dir `$IKSA_HOME` (default `~/.iksa`), mode 0700: `config.json`, `ledger.db`, `cas/`, `chromium/`, `profile/`.
- Launch args (exact, shell form, `cdp::launch` passes them verbatim): `--user-data-dir=<profile>`, `--disable-background-networking`, `--disable-component-update`, `--disable-sync`, `--no-pings`, `--disable-domain-reliability`, `--disable-field-trial-config`, `--disable-breakpad`, `--metrics-recording-only`, `--no-default-browser-check`, `--no-first-run`, `--safebrowsing-disable-auto-update`, `--disable-features=PreloadTopChromeWebUI,WebUIOmniboxPopup,WebUIOmniboxAimPopup`; plus `--use-mock-keychain` only when `mock_keychain` is true. Never `--enable-automation`, never `--remote-debugging-port`.
- Dependencies: only the Tech Stack list.
- Tests: unit tests in-module; Chromium tests `#[ignore]` in `tests/`, run with `CHROMIUM=$PWD/.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium cargo test --release -- --ignored --test-threads=1`. **A task is not done until its ignored tests have been run and pass.**

## Review Focus

- **Chromium crashes mid-call**: pending calls fail fast with `browser closed`, no hang. Test `browser_exit_fails_pending` (Task 1).
- **Process killed mid-task**: next `iksa run` requeues it at its saved cursor. Test `recover_requeues_executing` (Task 6).
- **Secret typed into a form**: never in record JSON, report, or DB. Test `secret_never_rendered` (Task 7).
- **Selector never appears / page hangs**: step times out, task `Failed("timeout at step N")`, report still renders. Test `hung_page_fails_with_timeout` (Task 8).
- **Turnstile that auto-passes**: agent continues instead of pausing. Test `turnstile_auto_pass_continues` (Task 8).

## Running this plan with Codex (gpt-6.1-sol)

One task per Codex run, in order. Codex needs full access (network + launching Chromium). Prompt template:

```
Read docs/superpowers/plans/2026-10-28-m1-agent-core-v2.md. Do Task <N> only, TDD as written: failing tests first, then code, then all listed commands including the #[ignore] ones. Follow Global Constraints exactly. If anything is blocked, write BLOCKED with the exact error; never fake a result or weaken a test. Stop after the task's commit and print that task's rows from the Validation checklist with actual results.
```

After each run, Claude reviews the diff against the task before the next one starts.

---

### Task 0: Close out spikes, rename docs

**Files:** rename `gemini-code-1793212240267.md` -> `PRD.md`, `gemini-code-1793212267498.md` -> `CLAUDE.md`; delete `spikes/src/`, `spikes/Cargo.toml`, `spikes/Cargo.lock`, `spikes/capture_window.py`, `docs/briefs/BRIEF_1_RESULT.md`; rewrite `spikes/README.md`; fill `CLAUDE.md`.

- [x] **Step 1:** `mv` the two docs (they're untracked), `git add PRD.md CLAUDE.md`.
- [x] **Step 2:** `git rm -r` the files above. `spikes/README.md` becomes ≤ 8 lines: how to run `raw_probe.py s3|s5`, and that the whole `spikes/` dir is deleted at the end of Task 8.
- [x] **Step 3:** `CLAUDE.md`: one-paragraph architecture (this plan's Architecture), module map (`cdp`, `cas`, `snapshot`, `config`, `gate`, `action`, `challenge`, `ledger`, `audit`, `engine`, `setup`, `main`), the test commands from Global Constraints, the rule "ignored tests must be run before a task is done", and "Codex implements; Claude plans and reviews".
- [x] **Step 4:** Commit `chore: close spike 1/2, adopt PRD.md and CLAUDE.md`.

### Task 1: Crate scaffold + pipe CDP client

**Files:** Create `Cargo.toml`, `src/main.rs` (`mod cdp; fn main() {}`), `src/cdp.rs`, `tests/common/mod.rs`, `tests/cdp.rs`; `.gitignore` adds `/target`.

**Interfaces — produces `cdp`:**
- `pub struct Event { pub method: String, pub params: Value, pub session_id: Option<String> }` (Clone)
- `pub struct Browser` — owns the child; `Drop` kills and reaps it.
- `pub fn launch(exe: &Path, args: &[String]) -> anyhow::Result<Browser>` — appends `--remote-debugging-pipe`; child fd 3 reads our requests, fd 4 writes responses (`std::io::pipe()` + `libc::dup2` in `CommandExt::pre_exec`); a `std::thread` reads fd 4.
- `Browser::call(&self, method: &str, params: Value, session: Option<&str>) -> anyhow::Result<Value>` — CDP error -> `Err` containing its `code` and `message`; EOF -> every pending call gets `Err("browser closed")`.
- `Browser::events(&self) -> tokio::sync::broadcast::Receiver<Event>` (capacity 1024). Callers subscribe **before** the call whose events they need.
- `Browser::pid(&self) -> u32`
- `pub(crate) fn split_frames(buf: &mut Vec<u8>) -> Vec<Vec<u8>>` — drains complete NUL-terminated frames, keeps the partial tail.
- `tests/common/mod.rs`: `chromium() -> PathBuf` (env `CHROMIUM`, panic with a clear message if unset), `temp_profile() -> PathBuf` (fresh 0700 dir under `std::env::temp_dir()`), `test_args(profile) -> Vec<String>` (Global Constraints list + `--use-mock-keychain`), `serve(routes: &[(&str, Response)]) -> u16` (~25-line `TcpListener` thread; `Response { status: u16, headers: Vec<(String,String)>, body: String, hang: bool }`).

- [x] **Step 1: Failing unit tests (`src/cdp.rs`):** `split_frames_partial` (`b"{\"a\":1}\0{\"b\""` -> one frame, tail `{"b"` kept), `split_frames_many` (3 frames -> 3, buffer empty), `split_frames_none` (no NUL -> nothing, buffer unchanged).
- [x] **Step 2: Failing integration tests (`tests/cdp.rs`, `#[ignore]`):**
  - `version_over_pipe`: `Browser.getVersion` -> `product` starts with `Chrome/156`.
  - `no_tcp_listener`: `lsof -nP -a -p <pid> -iTCP -sTCP:LISTEN` prints nothing.
  - `hidden_target_navigate_eval_type`: context; hidden target; attach flatten; `Page.enable`; subscribe; `Page.navigate` to the fixture form (`<input id=q><button id=go onclick="out.textContent=q.value"><output id=out>`); wait for that session's `Page.loadEventFired` (5 s); focus `#q`; `Input.insertText("pipe typed")`; JS click `#go`; `#out` == `"pipe typed"`.
  - `large_eval_roundtrip`: `'x'.repeat(2_000_000)` with `returnByValue` -> length 2,000,000.
  - `cdp_error_is_err`: `call("Nope.nope", json!({}), None)` -> `Err` containing `-32601`.
  - `browser_exit_fails_pending`: SIGKILL the pid; next call -> `Err` containing `browser closed` within 2 s.
  - `drop_kills_browser`: drop `Browser`; within 2 s `kill(pid, 0)` fails.
- [x] **Step 3:** `cargo test cdp` + the ignored command for `--test cdp` — FAIL.
- [x] **Step 4:** Implement.
- [x] **Step 5:** Same commands — 3 + 7 pass.
- [x] **Step 6:** Commit `feat: pipe-transport CDP client`.

### Task 2: CAS blob store

**Files:** `src/cas.rs`.

**Interfaces:** `Cas::open(root) -> io::Result<Cas>` (creates root 0700); `put(&[u8]) -> io::Result<blake3::Hash>` (idempotent, zstd level 3, write `tmp-<random>` then `rename`); `get(&Hash) -> io::Result<Vec<u8>>` (`InvalidData` on hash mismatch); `delete(&Hash)`; `all() -> io::Result<Vec<Hash>>`; `disk_bytes() -> io::Result<u64>`. Layout `root/ab/cdef…`.

- [ ] **Step 1: Failing tests:** `put_get_roundtrip` (hash == `blake3::hash(b"hello")`), `put_is_deduplicated` (second 100 KB put leaves `disk_bytes` unchanged), `corrupt_blob_detected` (`InvalidData`), `missing_blob_is_not_found` (`NotFound`), `root_is_owner_only` (mode & 0o777 == 0o700).
- [ ] **Step 2:** `cargo test cas` — FAIL. **Step 3:** implement. **Step 4:** 5 pass.
- [ ] **Step 5:** Commit `feat: CAS blob store (BLAKE3 + zstd)`.

### Task 3: DOM snapshot chunking

**Files:** `src/snapshot.rs`.

**Interfaces:** `chunks(html: &str) -> Vec<&str>`; `put(cas, html) -> io::Result<Hash>` (chunk blobs + tree blob `"tree\n"` + one hex hash per line); `get(cas, &tree) -> io::Result<String>`; `chunk_hashes(cas, &tree) -> io::Result<Vec<Hash>>`. Cut after a `>` byte when chunk ≥ 1024 bytes and `blake3::hash(last 16 bytes).as_bytes()[0] < 8`; force-cut at 16384.

- [ ] **Step 1: Failing tests** (fixture: 2,000 `<div class="row" id="r{i}">item {i}</div>` rows): `chunks_concat_to_input` (+ none > 16384), `small_edit_shares_chunks` (≥ 90% shared after inserting `<b>x</b>` mid-page), `roundtrip`, `ten_step_task_under_500kb`, `put_overhead_under_25ms` (`#[ignore]`, release).
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** `cargo test snapshot` + `cargo test --release snapshot -- --ignored` pass.
- [ ] **Step 5:** Commit `feat: content-defined DOM snapshot chunking`.

### Task 4: Config, search providers, gate, actions

**Files:** `src/config.rs`, `src/gate.rs`, `src/action.rs`.

**Interfaces:**
- `enum Risk { Read = 0, Write = 1, Financial = 2 }` (serde lowercase, `Ord`); `enum Decision { Auto, NeedsApproval }`; `gate::decide(auto_max: Risk, r: Risk) -> Decision` (`Financial` always `NeedsApproval`).
- `struct SearchProvider { name, url_template }`; `struct Config { search: String, providers: Vec<SearchProvider>, auto_max: Risk, retention_days: u32 }`, `deny_unknown_fields`. Default providers: DuckDuckGo `https://duckduckgo.com/?q={q}`, Kagi `https://kagi.com/search?q={q}`, Brave `https://search.brave.com/search?q={q}`, SearXNG `https://searx.be/search?q={q}`; `search: "DuckDuckGo"`, `auto_max: Read`, `retention_days: 30`.
- `Config::load(home) -> anyhow::Result<Config>` (missing file -> write default; errors: active provider not listed, template without `{q}`); `Config::search_url(&self, q) -> String` (percent-encode everything outside `A-Za-z0-9-._~`, no crate).
- `Action` (`#[serde(tag = "op", rename_all = "snake_case")]`): `Navigate { url }`, `Search { query }`, `Click { selector, risk = Write }`, `Type { selector, text, secret = false, risk = Write }`, `Extract { selector }`; `risk()` (Navigate/Search/Extract = Read); `redacted()` (secret text -> `"••••"`).

- [ ] **Step 1: Failing tests:** `read_auto_write_needs_approval_by_default`; `financial_always_needs_approval` (`decide(Financial, Financial) == NeedsApproval`, `decide(Financial, Write) == Auto`); `default_config_written_and_reloaded`; `kagi_search_url` (`search_url("rust & c++") == "https://kagi.com/search?q=rust%20%26%20c%2B%2B"`); `custom_provider_without_q_rejected`; `unknown_active_provider_rejected`; `unknown_config_field_rejected`; `action_json_shape`; `redacted_hides_secret`.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** pass.
- [ ] **Step 5:** Commit `feat: config, search providers, risk gate, actions`.

### Task 5: Challenge detection

**Files:** `src/challenge.rs`.

**Interfaces:** `enum ChallengeKind { CloudflareChallenge, Turnstile, HCaptcha, ReCaptcha }` (serde); `detect(status: Option<u16>, headers: &[(String, String)], html: &str) -> Option<ChallengeKind>`. First match wins, header names case-insensitive:
1. `cf-mitigated: challenge`, or status 403/503 + `server: cloudflare` + html contains `Just a moment` -> `CloudflareChallenge`
2. `challenges.cloudflare.com/turnstile` or `class="cf-turnstile"` -> `Turnstile` (the engine then checks for a token; see Task 8)
3. `hcaptcha.com/` or `class="h-captcha"` -> `HCaptcha`
4. `google.com/recaptcha` or `class="g-recaptcha"` -> `ReCaptcha`

- [ ] **Step 1: Failing tests:** `cf_header`, `cf_interstitial`, `turnstile_iframe`, `hcaptcha_div`, `recaptcha_script`, `plain_403_is_not_challenge`, `article_mentioning_captcha_is_not_challenge`.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** pass.
- [ ] **Step 5:** Commit `feat: challenge detection`.

### Task 6: Ledger (SQLite)

**Files:** `src/ledger.rs`.

**Interfaces:**
- `enum Handoff { Approval, Challenge(ChallengeKind) }`; `enum State { Queued, Executing, AwaitingHandoff(Handoff), Committed, Failed(String) }` (stored as JSON text).
- `struct Task { id: i64, title: String, steps: Vec<Action>, cursor: usize, state: State, approved_step: Option<usize>, finished_ms: Option<u64> }`.
- `enum LedgerError { Sql, Json, NotFound(i64), Illegal { from: State, to: State } }` (thiserror, `#[from]` for the first two).
- `Ledger::open(path)`, `add(title, &[Action]) -> i64`, `get(id)`, `list()`, `set_cursor(id, usize)`, `next_queued() -> Option<Task>` (lowest id).
- `transition(id, to)`: legal `Queued->Executing`; `Executing->{AwaitingHandoff, Committed, Failed, Queued}`; `AwaitingHandoff->{Queued, Failed}`; else `Illegal`. Sets `finished_ms` on `Committed`/`Failed`.
- `approve(id)` only from `AwaitingHandoff(Approval)`: `approved_step = Some(cursor)`, `Queued`. `resume(id)` only from `AwaitingHandoff(Challenge(_))`: `Queued`. `recover() -> usize`: all `Executing` -> `Queued`.

- [ ] **Step 1: Failing tests:** `add_then_get`, `happy_path_transitions`, `committed_is_terminal`, `approve_only_from_approval_wait`, `recover_requeues_executing` (cursor kept across reopen), `finished_ms_set_on_commit`.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** pass.
- [ ] **Step 5:** Commit `feat: SQLite task ledger`.

### Task 7: Audit records, report, retention GC

**Files:** `src/audit.rs`; modify `src/ledger.rs` (`steps(task_id, step, json)` table).

**Interfaces:**
- `struct StepRecord { step: usize, action: Action, decision: String /* "auto"|"approved" */, after: Option<String> /* tree hex */, outcome: String, at_ms: u64 }`. A step's "before" is the previous record's `after`.
- `StepRecord::new(step, &Action, decision, after, outcome)` stores `action.redacted()` and forces `after = None` for `Financial` risk.
- `Ledger::record_step(id, &StepRecord)`, `Ledger::records(id) -> Vec<StepRecord>`, `Ledger::delete_task(id)`.
- `audit::render(&Task, &[StepRecord], &Cas) -> io::Result<String>`: `# Task {id}: {title}`, state line, per step `## Step N — {op}`, action JSON, decision, outcome (≤ 2,000 chars), `DOM: {changed}/{total} chunks changed` vs previous snapshot, or `DOM: not stored`.
- `audit::gc(&Ledger, &Cas, retention_days, now_ms) -> anyhow::Result<(usize, usize)>`: drop finished tasks older than retention, then mark-and-sweep unreachable blobs.

- [ ] **Step 1: Failing tests:** `report_lists_every_step`, `secret_never_rendered`, `financial_step_stores_no_snapshot`, `records_roundtrip`, `dom_change_summary`, `gc_removes_old_tasks_and_orphan_blobs` (shared blobs kept, running task untouched).
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** pass.
- [ ] **Step 5:** Commit `feat: audit records, report, retention GC`.

### Task 8: Engine on `cdp::Browser` + pillar tests

**Files:** `src/engine.rs`, `tests/engine.rs`, `tests/pillars.rs`; extend `tests/common/mod.rs` with `tree_footprint_kib(pid) -> u64` (sum of `footprint -p <pid> -f bytes` `phys_footprint` over the pid and all descendants from `ps -axo pid=,ppid=`, ÷ 1024) and `netlog_hosts(path) -> Vec<String>` (every `params.url` host, `params.host`, `params.address` host across all events, loopback filtered out).

**Interfaces:**
- `engine::launch_args(profile: &Path, mock_keychain: bool) -> Vec<String>` — exactly the Global Constraints list.
- `Engine::launch(chromium: &Path, profile: &Path, mock_keychain: bool) -> anyhow::Result<Engine>` (profile dir created 0700).
- `Engine::run(&self, &Ledger, &Cas, &Config, id) -> anyhow::Result<State>`; `Engine::run_with_timeout(.., step_timeout: Duration)`; `const STEP_TIMEOUT: Duration = Duration::from_secs(30)`; `const TURNSTILE_WAIT: Duration = Duration::from_secs(10)`.

How `run` maps to CDP (algorithm, since the signatures don't fix it):
1. `transition(Executing)`; `Target.createBrowserContext`; hidden target in it; attach flatten; `Page.enable`, `Network.enable`, `Emulation.setDeviceMetricsOverride { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false }`. If cursor > 0, re-navigate to the last `Navigate`/`Search` URL before cursor.
2. Per step from cursor: `gate::decide(cfg.auto_max, action.risk())`; `NeedsApproval` and `approved_step != Some(cursor)` -> `AwaitingHandoff(Approval)`, return.
3. Run the action under the step timeout:
   - Navigate/Search (`cfg.search_url`): subscribe; `Page.navigate` (non-empty `errorText` -> error); take status + headers from the `Network.responseReceived` with `type == "Document"` and the returned `loaderId`; wait for `Page.loadEventFired` on the session.
   - Locate `selector`: poll every 100 ms with `Runtime.evaluate` until `document.querySelector(sel)` exists, then `scrollIntoView({block:"center"})` and return its rect center.
   - Click: `Input.dispatchMouseEvent` `mousePressed` + `mouseReleased` at the center (`button: "left"`, `clickCount: 1`).
   - Type: Click, then `Input.insertText(text)`.
   - Extract: located element's `innerText` -> outcome.
4. One snapshot after the action: `document.documentElement.outerHTML` -> `snapshot::put` (skipped for Financial); `record_step`.
5. `challenge::detect(..)`. For `Turnstile`, poll `document.querySelector('[name="cf-turnstile-response"]')?.value` every 250 ms for up to `TURNSTILE_WAIT`; non-empty -> not a challenge. Any remaining kind -> `AwaitingHandoff(Challenge(k))`, return without advancing the cursor.
6. Error/timeout: record outcome `error: …` / `timeout at step N`; `Failed(..)`; return.
7. `set_cursor(cursor + 1)`; after the last step `Committed`. Always `Target.disposeBrowserContext`.

Fixture routes (`common::serve`): `/form` (`<h1>Form</h1>` + the Task 1 form), `/cf` (403, `cf-mitigated: challenge`), `/slow` (hang), `/search?q=…` (echoes decoded q in `<h1>`), `/turnstile` (`<div class="cf-turnstile"></div><input name="cf-turnstile-response">`, no token), `/turnstile-auto` (same, plus a script that sets the input's `.value = "tok"` after 1 s).

- [ ] **Step 1: Failing tests.**
  - Unit: `launch_args_exact` (contains `--disable-features=PreloadTopChromeWebUI,WebUIOmniboxPopup,WebUIOmniboxAimPopup` and all privacy flags; no `--enable-automation`, no `remote-debugging-port`); `mock_keychain_only_when_asked` (absent when `false`).
  - `tests/engine.rs` (`#[ignore]`): `runs_read_task_to_committed` (Extract `h1` == "Form"); `search_uses_configured_provider` (provider `Local` -> fixture `/search?q={q}`; Search "a b"; Extract `h1` == "a b"); `write_step_waits_for_approval` (Type -> `AwaitingHandoff(Approval)`, cursor 1; `approve`; run -> `Committed`; Extract shows typed value); `click_uses_real_input` (Type "hi" into `#q` + Click `#go` with `auto_max: Write` -> `#out` == "hi"); `challenge_pauses` (`/cf` -> `AwaitingHandoff(Challenge(CloudflareChallenge))`); `turnstile_blocks_without_token` (`/turnstile` -> `AwaitingHandoff(Challenge(Turnstile))`); `turnstile_auto_pass_continues` (`/turnstile-auto` then Extract -> `Committed`); `hung_page_fails_with_timeout` (`run_with_timeout` 2 s on `/slow` -> `Failed` containing `timeout`; `render` works); `contexts_are_isolated` (cookie from task A invisible to task B); `step_overhead_under_25ms` (median over 20 Extract steps of step wall time minus time spent inside `call`, < 25 ms).
  - `tests/pillars.rs` (`#[ignore]`, need network): `netlog_sees_real_navigation` (launch with `--log-net-log=<tmp>`; hidden tab navigates `https://example.com`; close; `netlog_hosts` contains `example.com`) — **the positive control; if it fails, the next test means nothing**; `idle_egress_is_loopback_only` (same launch, idle 60 s on `about:blank`, `netlog_hosts` empty); `idle_footprint_under_300mb` (median of 3 fresh launches, 10 s settle, `tree_footprint_kib` < 307,200); `five_tabs_three_agents_under_1200mb` (5 visible targets + 3 hidden agent contexts on Wikipedia articles `Rust_(programming_language)`, `Linux`, `Chromium_(web_browser)`, `SQLite`, `Mozilla`, `Python_(programming_language)`, `Tokio_(software)`, `WebKit`; 20 s settle; < 1,228,800).
- [ ] **Step 2:** `cargo test engine` + ignored `--test engine` and `--test pillars` — FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Same commands — all pass. Record the numbers from the pillar tests (footprints, hosts) in `docs/perf.md`.
- [ ] **Step 5:** `git rm -r spikes/` (the Rust tests replace it); update `CLAUDE.md` module map.
- [ ] **Step 6:** Commit `feat: CDP engine with gates, snapshots, challenge pause; pillar tests`.

### Task 9: CLI, setup, end to end

**Files:** `src/main.rs`, `src/setup.rs`, `tests/cli.rs`, `examples/hn.json` (Search "hacker news", Navigate `https://news.ycombinator.com`, Extract `.titleline`).

**Interfaces:**
- `setup::PIN` = the spike pin (arm64 only). `setup::chromium_path(home) -> Option<PathBuf>` (`$CHROMIUM` if set, else the installed binary). `setup::install(home) -> anyhow::Result<PathBuf>`: `curl -fL`, `shasum -a 256` (mismatch -> delete file, `Err`), `hdiutil attach -nobrowse` + `cp -R` + detach, `xattr -dr com.apple.quarantine`. Idempotent: does nothing if the installed version matches `PIN`, reinstalls if not (no separate `update` command).
- Commands (`std::env::args` match): `setup`, `add <task.json>` (`{"title","steps"}`; prints id), `run` (`recover()`, `gc`, then each queued task), `list`, `approve <id>`, `resume <id>`, `report <id>`, `gc`. Bad usage: usage text on stderr, exit 2. Error: `error: …` on stderr, exit 1. Tests set `IKSA_MOCK_KEYCHAIN=1` -> `mock_keychain = true`; never set by default.

- [ ] **Step 1: Failing tests** (`env!("CARGO_BIN_EXE_iksa")`, temp `IKSA_HOME`): `add_and_list`; `bad_task_json_exits_nonzero` (`{"op":"fly"}` -> exit 1, `error:`, ledger unchanged); `bad_config_exits_nonzero` (template without `{q}` -> exit 1, names the provider); `chromium_launch_failure_keeps_task_queued` (`CHROMIUM=/nonexistent` -> exit 1, still `Queued`); `run_without_setup_says_run_setup` (stderr contains `iksa setup`); `checksum_mismatch_aborts_install` (unit in `setup.rs`); `e2e_report` (`#[ignore]`, network: add `examples/hn.json`, `run`, `report` contains `## Step 2` and `Committed`).
- [ ] **Step 2:** FAIL. **Step 3:** implement.
- [ ] **Step 4:** `cargo test` + full ignored command — all pass.
- [ ] **Step 5:** Pillar check into `docs/perf.md`: `ls -l target/release/iksa` (< 60 MB), `du -sk $IKSA_HOME/cas` after `e2e_report` (< 500), pillar test numbers from Task 8.
- [ ] **Step 6:** Commit `feat: iksa CLI, pinned Chromium setup, end to end`.

### Task 10: Manual checks (user, ~5 min, normal Terminal)

Results go in `docs/perf.md` under `## Manual`.

- [ ] **H1 Keychain:** `iksa setup && iksa run` with an empty task list from a normal Terminal. Expected: at most one "Chromium Safe Storage" prompt; after **Always Allow**, no prompt on the next run.
- [ ] **H2 Address bar with Task 8's flags:** launch the pinned Chromium with `engine::launch_args` + `--load-extension=<any search-override extension>`. Typing in the address bar shows a dropdown with suggestions, and Enter searches the overriding provider. Note whether a "Change search settings?" bubble appears.

## Validation checklist

| # | Task | Check | Pass |
| --- | --- | --- | --- |
| C1 | 0 | `git ls-files spikes/src spikes/Cargo.toml` | empty |
| C2 | 1 | `cargo test cdp` | 3 pass |
| C3 | 1 | ignored `--test cdp` | 7 pass incl. `no_tcp_listener`, `browser_exit_fails_pending` |
| C4 | 2 | `cargo test cas` | 5 pass |
| C5 | 3 | `cargo test snapshot` + release ignored | 5 pass; overhead < 25 ms |
| C6 | 4 | `cargo test config gate action` | 9 pass |
| C7 | 5 | `cargo test challenge` | 7 pass |
| C8 | 6 | `cargo test ledger` | 6 pass |
| C9 | 7 | `cargo test audit` | 6 pass |
| C10 | 8 | ignored `--test engine` | 10 pass |
| C11 | 8 | `netlog_sees_real_navigation` | `example.com` found |
| C12 | 8 | `idle_egress_is_loopback_only` | 0 hosts |
| C13 | 8 | `idle_footprint_under_300mb` | median < 300 MB (spike: 223) |
| C14 | 8 | `five_tabs_three_agents_under_1200mb` | < 1.2 GB |
| C15 | 9 | `cargo test --test cli` + `e2e_report` | 7 pass |
| C16 | 9 | binary size / CAS size | < 60 MB / < 500 KB |
| C17 | 10 | H1 keychain | ≤ 1 prompt, none after Always Allow |
| C18 | 10 | H2 address bar | dropdown works; bubble noted |
