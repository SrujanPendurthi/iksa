# iksa — M1 Agent Core v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Supersedes:** the v1 M1 plan and the Spike 2 plan (deleted in Task 2b; both are in git history at `e3c5fb5`).

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

- **iksa exits while Chromium runs**: Chromium closes cleanly (cookies and prefs flushed) and no helper process survives. Tests `drop_closes_gracefully`, `drop_leaves_no_helpers` (Task 2b).
- **Typo in a task's `risk` field** (e.g. `"rsik":"financial"`): task is rejected, never silently downgraded to `write` and auto-run. Test `action_unknown_field_rejected` (Task 5b).
- **Approval in the middle of a form**: after `approve`, typed fields are back before the approved click; if repeating an earlier click would be needed, the task fails safely instead. Tests `approval_resume_replays_typed_fields`, `resume_with_click_before_cursor_fails_safely` (Task 8b).
- **Chromium crashes mid-call**: pending calls fail fast with `browser closed`, no hang. Test `browser_exit_fails_pending` (Task 1).
- **Process killed mid-task**: next `iksa run` requeues it at its saved cursor. Test `recover_requeues_executing` (Task 6).
- **Secret typed into a form**: never in record JSON, outcomes, snapshots or report; erased from `ledger.db` and `ledger.db-wal` bytes once the task is `Committed` or `Failed`. Tests `secret_never_rendered`, `secret_erased_on_failed` (Task 7b).
- **Selector never appears / page hangs**: step times out, task `Failed("timeout at step N")`, report still renders. Test `hung_page_fails_with_timeout` (Task 8).
- **Turnstile that auto-passes**: agent continues instead of pausing. Test `turnstile_auto_pass_continues` (Task 8).

## Running this plan with Codex (gpt-6.1-sol)

One task per Codex run, in order. Codex needs full access (network + launching Chromium). Prompt template:

```
Read docs/superpowers/plans/2026-10-28-m1-agent-core-v2.md. Do Task <N> only, TDD as written: failing tests first, then code, then all listed commands including the #[ignore] ones. Follow Global Constraints exactly. If anything is blocked, write BLOCKED with the exact error; never fake a result or weaken a test. Stop after the task's commit and print that task's rows from the Validation checklist with actual results.
```

After each run, Claude reviews the diff against the task before the next one starts.

Notes for Codex runs (from Tasks 0–2):
- Your per-task scratch reports go in `.spike-home/` and are deleted in Task 2b. From Task 3 on, write scratch to `target/codex/` (gitignored via `/target`) instead.
- `cdp::Browser::call` writes synchronously under a mutex. Fine while Chromium is alive; if a step deadline fires because Chromium itself is wedged, the engine drops the `Browser` (Task 2b makes that a clean close + kill). Don't build a separate async writer.

---

## Done

| Task | Commit | What landed | Checks |
| --- | --- | --- | --- |
| 0 Close out spikes, rename docs | `4f82a25` | `PRD.md`, `CLAUDE.md`, spike Rust crate removed | C1 |
| 1 Pipe CDP client | `516da26` | `src/cdp.rs`: `launch`, `Browser::{call, events, pid}`, `Event`, `split_frames`; `tests/common/mod.rs` (`chromium`, `temp_profile`, `test_args`, `serve`, `Response`) | C2, C3 |
| 2 CAS blob store | `96193d0` | `src/cas.rs`: `Cas::{open, put, get, delete, all, disk_bytes}` | C4 |
| 2b Review fixes | `1a98892` | `src/lib.rs` (`pub mod cas; pub mod cdp;`), clean `Browser.close` in `Drop`, CAS `sync_all` + plain rename + self-heal on corrupt read, superseded plans and briefs deleted | C4b–C4e |
| 2c Review trims | `23ec1b6` | non-blocking close write, `exit_type == "Normal"` check, `common::descendants(pid)`, CAS trims | C4f, C4g |
| 3 Snapshot chunking | `ef56e64` | `src/snapshot.rs`: `chunks`, `put`, `get`, `chunk_hashes` | C5 |
| 3b Plain fsync | `e245cc3` | `Cas::put` uses `libc::fsync` (was `sync_all` = `F_FULLFSYNC`, ~4 ms/blob) | C5, C5b |
| 4 Config, search, gate, actions | `e451c1b` | `src/{config,gate,action}.rs`: `Config::{load, search_url}`, `gate::decide`, `Action::{risk, redacted}`; done by Claude while Codex quota was at 86% | C6 |
| 5 Challenge detection | `1439d7a` | `src/challenge.rs`: `ChallengeKind`, `detect` | C7 |
| 5b Review fixes | `8bd8d86` | `Action` `deny_unknown_fields`; `Config::load` creates home 0700, rejects `retention_days: 0`, `with_context` errors; tighter hCaptcha + case-insensitive headers; `src/testutil.rs` (`temp_dir(label) -> TempDir`) | C7b, C7c |
| 5c reCAPTCHA match | `1c04723` | rule 4 matches `/recaptcha/api.js`, `/recaptcha/enterprise.js`, `class="g-recaptcha"` | C7d |
| 6 SQLite task ledger | `5f45616` | `src/ledger.rs`: `Handoff`, `State`, `Task`, `LedgerError`, `Ledger::{open, add, get, list, set_cursor, next_queued, transition, approve, resume, recover}`; WAL + `synchronous=NORMAL` | C8 |
| 6b Ledger hardening | `0067be2` | conditional `UPDATE … AND state=?` in `transition`/`approve`, `fallible_uint` conversions, SQL `unixepoch`, `const SELECT`. (rusqlite 0.41 already sets a 5 s busy timeout; the explicit call stays.) | C8b, C8c |
| 7 Audit | `6ac1079` | `src/audit.rs`: `StepRecord { step, action, decision: String /* "auto"|"approved" */, after: Option<String> /* tree hex */, outcome, at_ms }`, `StepRecord::new(step, &Action, decision, after, outcome)` (redacts; no snapshot for Financial), `render(&Task, &[StepRecord], &Cas) -> io::Result<String>`, `gc(&Ledger, &Cas, retention_days, now_ms) -> anyhow::Result<(usize, usize)>`; `Ledger::{record_step, records, delete_task}`. C9 BLOCKED on secret in `tasks.steps` → Task 7b | C9 (5/6) |
| 7b Secrets erased, append-only audit | `b933cd9` | terminal `transition` rewrites `tasks.steps` redacted + `secure_delete` + `wal_checkpoint(TRUNCATE)`; `steps.seq` append-only, FK cascade; `gc`/`render` skip missing trees; `audit::now_ms()` | C9, C9b |
| 7c Non-fatal checkpoint | `1d443eb` | busy WAL checkpoint returns `Ok`; checkpoint on `open`/`Drop`; `Cas::path` `pub(crate)`; audit test `setup()` | C9d |
| 8 Engine + pillar tests | `cb51b42` | `src/engine.rs`: `launch_args(profile, mock_keychain)`, `Engine::{launch, run, run_with_timeout}`, `STEP_TIMEOUT`, `CAPTCHA_WAIT`; `tests/{engine,pillars}.rs`; `spikes/` removed; numbers in `docs/perf.md` | C10–C14 |

New modules go in `src/lib.rs`; `main.rs` is the CLI only. Integration tests `use iksa::…`.

### Task 8b: Engine fixes (resume, secrets in snapshots) + trims

From Claude's caveman and ponytail reviews of `cb51b42`. Two bugs must be fixed before Task 9 builds the CLI on the engine.

**Bug 1 — approval resume loses typed fields.** Each run uses a fresh browser context and replays only the last `Navigate`/`Search` before the cursor. Login flow `Navigate, Type email, Type password (secret), Click #login (Financial)`: it pauses at the click, and after `approve` the resumed run clicks login on an empty form.
**Bug 2 — secrets leak into snapshots and outcomes.** After `Type { secret: true }` the engine stores `outerHTML`; pages that mirror input (our `/form` fixture, strength meters, frameworks that set the `value` attribute) put the secret into the CAS for up to `retention_days`. A later `Extract` of a mirrored element also writes it into the record's `outcome`.

**Files:** `src/engine.rs`, `src/audit.rs` (one ignored test), `tests/engine.rs`, `tests/common/mod.rs`.

**Interfaces / rules:**
- `fn url_of(a: &Action, cfg: &Config) -> Option<String>` (Navigate → url, Search → `cfg.search_url(query)`), used by `action()` and by resume.
- Resume (cursor > 0): let `nav` = index of the last step before the cursor where `url_of` is `Some`. Navigate to it, then re-run every `Type` in `nav+1..cursor` (no records, no gate). If any `Click` lies in that range, end with `Failed("cannot resume: step {i} would repeat a click")` and record it.
- Secret hygiene: the engine keeps `secrets: Vec<&str>` = the `text` of every `Type { secret: true }` in the task. A `secret_on_page` flag is set when a secret `Type` runs and cleared when a `Navigate`/`Search` completes a fresh Document navigation (review privacy ruling: same-document navigation preserves it). While it is set (including the secret step itself), store no snapshot (`after = None`). Every `outcome` and failure message passes through `scrub(s, &secrets)`, which replaces each secret with `"••••"` before `record_step`.
- Same-document navigation: `Page.navigate` with no `loaderId` and empty `errorText` returns `Ok((None, vec![]))` immediately.
- `eval` errors include `exceptionDetails.exception.description` (or `.text`).
- One failure path: the step loop returns `Err` for timeout/error; only the outer handler records the failure and transitions to `Failed`. Its record's `decision` is `"approved"` if `approved_step == Some(cursor)`, else `"auto"`.
- Headers: `serde_json::from_value::<HashMap<String, String>>(r["headers"].clone()).unwrap_or_default().into_iter().collect()`.
- Remove the timing instrumentation from `Engine` (`call_time`, `overheads`, `step_overheads()`, the `call()` wrapper — call `b.call` directly; `Engine` becomes `{ browser: RefCell<Option<Browser>> }`). The 25 ms check moves out of `tests/engine.rs` (delete `step_overhead_under_25ms`) to `audit::tests::step_bookkeeping_under_25ms` (`#[ignore]`, release): 20 distinct ~80 KB fixture pages, each `snapshot::put` + `record_step` + `set_cursor` timed; median < 25 ms.
- `tests/common::test_args` is deleted; tests call `iksa::engine::launch_args(&profile, true)`. `tree_footprint_kib` keeps parse + sum + assert only (no files written to `target/codex`, no `println!`).

- [x] **Step 1: Failing tests (`tests/engine.rs`, `#[ignore]`):**
  - `approval_resume_replays_typed_fields`: steps Navigate `/form`, Type `#q` "hi", Click `#go` (`risk: financial`), default `auto_max` raised to `Write`; run → `AwaitingHandoff(Approval)` at cursor 2; `approve`; run → `Committed`; a final Extract step `#out` == "hi".
  - `resume_with_click_before_cursor_fails_safely`: Navigate `/form`, Click `#go` (auto), Type `#q` "x" (Financial-risk `Type`); run → approval at cursor 2; `approve`; run → `Failed` containing `would repeat a click`.
  - `secret_never_in_snapshots_or_outcomes`: `auto_max: Write`; Navigate `/form`, Type `#q` secret "hunter2", Click `#go`, Extract `#out`, Navigate `/form`, Extract `h1` → `Committed`; no blob from `cas.all()` contains `hunter2`; `records` JSON doesn't contain it; the `#out` Extract outcome is `"••••"`; the records for steps 1–3 have `after == None`, and the step 4 and 5 records have `Some`.
  - `same_document_navigation_commits`: Navigate `/form`, Navigate `/form#x`, Extract `h1` → `Committed`.
  - `bad_selector_error_is_explained`: Extract `##bad` → `Failed` whose message contains `SyntaxError` or `not a valid selector`.
- [x] **Step 2:** `cargo test` + ignored `--test engine` — the 5 new tests fail (`same_document…` and `bad_selector…` may already pass partially; they guard the change).
- [x] **Step 3:** Implement the rules above; `rm -rf .spike-home/Library` (untracked leftovers).
- [x] **Step 4:** `cargo test` + `CHROMIUM=… cargo test --release -- --ignored --test-threads=1` — all pass, including `step_bookkeeping_under_25ms` and the 4 pillar tests. Update `docs/perf.md` C10 with the new bookkeeping median, and add a "Watch (M3)" line: C14's worst sample was 1,206,946 KiB = 98% of budget.
- [x] **Step 5:** Commit `fix: resume replays typed fields, secret-free snapshots and outcomes; engine trims`.

### Task 9: CLI, setup, end to end

**Files:** `src/main.rs`, `src/setup.rs`, `tests/cli.rs`, `examples/hn.json` (Search "hacker news", Navigate `https://news.ycombinator.com`, Extract `.titleline`).

**Interfaces:**
- `setup::PIN` = the spike pin (arm64 only). `setup::chromium_path(home) -> Option<PathBuf>` (`$CHROMIUM` if set, else the installed binary). `setup::install(home) -> anyhow::Result<PathBuf>`: `curl -fL`, `shasum -a 256` (mismatch -> delete file, `Err`), `hdiutil attach -nobrowse` + `cp -R` + detach, `xattr -dr com.apple.quarantine`. Idempotent: does nothing if the installed version matches `PIN`, reinstalls if not (no separate `update` command).
- Commands (`std::env::args` match): `setup`, `add <task.json|->` (`{"title","steps"}`; `-` reads stdin so a secret never has to sit in a file; prints id; if a file path was given and any step is `Type { secret: true }`, also print to stderr `warning: <path> contains a secret; delete it — iksa keeps its own copy until the task finishes`), `run` (`recover()`, `gc`, then each queued task), `list`, `approve <id>`, `resume <id>`, `report <id>`, `gc`. Bad usage: usage text on stderr, exit 2. Error: `eprintln!("error: {e:#}")` (alternate format, so `with_context` errors keep their cause), exit 1. Before anything else `main` creates `IKSA_HOME` with mode 0700 and, if it already exists, `set_permissions` it to 0700. Tests set `IKSA_MOCK_KEYCHAIN=1` -> `mock_keychain = true`; never set by default.

- [ ] **Step 1: Failing tests** (`env!("CARGO_BIN_EXE_iksa")`, temp `IKSA_HOME`): `add_and_list`; `bad_task_json_exits_nonzero` (`{"op":"fly"}` -> exit 1, `error:`, ledger unchanged); `bad_config_exits_nonzero` (template without `{q}` -> exit 1, stderr names the provider and contains `{q}`); `unknown_config_field_shows_reason` (config with `"bogus": 1` -> exit 1, stderr contains both `config.json` and `bogus`); `existing_home_tightened_to_0700` (pre-create `IKSA_HOME` at 0755, run `list`, mode is 0700); `add_from_stdin` (pipe the task JSON into `add -` → `list` shows it); `add_secret_file_warns` (file with a secret `Type` → exit 0, stderr contains `contains a secret`); `run_after_crash_recovers_then_gcs` (leave a task `Executing` in the ledger, then `run` with `CHROMIUM=/nonexistent` → `list` shows it `Queued`, and GC did not refuse); `chromium_launch_failure_keeps_task_queued` (`CHROMIUM=/nonexistent` -> exit 1, still `Queued`); `run_without_setup_says_run_setup` (stderr contains `iksa setup`); `checksum_mismatch_aborts_install` (unit in `setup.rs`); `e2e_report` (`#[ignore]`, network: add `examples/hn.json`, `run`, `report` contains `## Step 2` and `Committed`).
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
| C1 | 0 | `git ls-files spikes/src spikes/Cargo.toml` | empty — **done** |
| C2 | 1 | `cargo test cdp` | 3 pass — **done** |
| C3 | 1 | ignored `--test cdp` | 7 pass incl. `no_tcp_listener`, `browser_exit_fails_pending` — **done** |
| C4 | 2 | `cargo test cas` | 5 pass — **done** |
| C4b | 2b | `cargo test` + ignored `--test cdp` | unit all pass; 9 cdp pass incl. `drop_closes_gracefully`, `drop_leaves_no_helpers` — **done** |
| C4c | 2b | `grep -c 'renamex_np\|urandom\|#\[path' src/cas.rs tests/cdp.rs` | 0 and 0 — **done** |
| C4d | 2b | `git ls-files docs/briefs docs/superpowers/plans` | only `2026-10-28-m1-agent-core-v2.md` — **done** |
| C4e | 2b | `du -sh spikes/target .spike-home` | `spikes/target` gone; `.spike-home` ≈ chromium + logs — **done** |
| C4f | 2c | `cargo test` + ignored `--test cdp` | 8 unit, 9 cdp pass; `drop_closes_gracefully` sees `exit_type == "Normal"` — **done** |
| C4g | 2c | `grep -c 'std::time::\|to_str()' src/cdp.rs src/cas.rs` | 0 and 0 — **done** |
| C5 | 3 | `cargo test snapshot` + release ignored | 5 pass; overhead < 25 ms — **done** (median 8.5 ms) |
| C5b | 3b | `grep -c 'sync_all(' src/cas.rs`; release ignored snapshot test | 0 calls; median of 5 fresh puts < 25 ms — **done** (8.5 ms) |
| C6 | 4 | `cargo test --lib -- config:: gate:: action::` | 9 pass — **done** |
| C7 | 5 | `cargo test challenge` | 7 pass — **done** |
| C7b | 5b | `cargo test --lib` | 5 new tests pass; total lib = previous + 5 — **done** |
| C7c | 5b | `grep -rn 'struct Temp(\|struct Home\|target/codex' src/` | no matches — **done** |
| C7d | 5c | `cargo test --lib -- challenge::` | 3 new pass (`recaptcha_about_link_is_not_challenge`, `recaptcha_net_script`, `recaptcha_enterprise_script`) — **done** |
| C8 | 6 | `cargo test ledger` | 6 pass — **done** |
| C8b | 6b | `cargo test --lib -- ledger::` | 9 pass incl. `concurrent_writer_waits`, `stale_handle_transition_rejected`, `finished_ms_is_epoch_ms` — **done** |
| C8c | 6b | `grep -c 'try_from\|SystemTime'` in `src/ledger.rs` non-test code | 0 — **done** |
| C9 | 7 | `cargo test audit` | 6 pass — 5/6, `secret_never_rendered` BLOCKED (spec), moved to 7b; **RESOLVED in 7b: 10/10 pass under finish-time erasure contract** — **done** |
| C9b | 7b | `cargo test --lib -- audit:: ledger::` | 20/20 pass (10 audit + 10 ledger), incl. `secret_never_rendered`, `secret_erased_on_failed`, `rerun_step_keeps_both_records`, `gc_survives_missing_tree`, `render_survives_missing_tree`, `delete_task_cascades` — **done** |
| C9d | 7c | `cargo test --lib -- ledger::terminal_transition_ok_while_reader_open` + full suite | pass; no regressions |
| C10 | 8 | ignored `--test engine` | 11 pass; unique-snapshot fsync overhead median 0.394916 ms — **done** |
| C11 | 8 | `netlog_sees_real_navigation` | 200 Example Domain, `example.com` found before C12 — **done** |
| C12 | 8 | `idle_egress_is_loopback_only` | 0 hosts after 60 s — **done** |
| C13 | 8 | `idle_footprint_under_300mb` | samples 213511/213911/214695 KiB; median 213911 < 307200 KiB — **done** |
| C14 | 8 | `five_tabs_three_agents_under_1200mb` | samples 1206946/1167745/1143666 KiB; median 1167745 < 1228800 KiB; all individual checks pass — **done** |
| C14b | 8b | ignored `--test engine` | 15/15 pass (11 − old overhead + 5 new); all 5 new tests first failed; full release ignored 30/30 pass — **done** |
| C14c | 8b | release ignored `audit::tests::step_bookkeeping_under_25ms` | 20 distinct 82,526-byte pages; median 6.336604 ms < 25 ms — **done** |
| C14d | 8b | `grep -c 'call_time\|step_overheads\|fn test_args\|target/codex' src/engine.rs tests/common/mod.rs` | 0 and 0 — **done** |
| C15 | 9 | `cargo test --test cli` + `e2e_report` | 12 pass |
| C16 | 9 | binary size / CAS size | < 60 MB / < 500 KB |
| C17 | 10 | H1 keychain | ≤ 1 prompt, none after Always Allow |
| C18 | 10 | H2 address bar | dropdown works; bubble noted |


**C5/C5b:** done in `e245cc3`; median 8.5 ms (was 238 ms with `sync_all`). The old C5b grep counted the required comment; fixed to count calls.
