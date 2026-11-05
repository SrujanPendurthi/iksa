# iksa — M2b Review Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply the 2026-11-05 caveman review (correctness) and ponytail review/audit (complexity) of commits `71016fa`..`c2e1e4b`, so M2b closes with pause, grants and the extension launch behaving as a user expects, and the tree gets shorter.

**Architecture:** No new features.
- Task 0 fixes correctness: pause semantics, grant CLI and the extension-launch failure.
- Task 1 deletes duplicate docs and tests.
- Task 2 makes the code cuts.

**When:** after M3 Task 1 (control socket) is committed and before M3 Task 2 starts. Task 0 touches `src/daemon.rs`, which M3 Task 1 is editing; don't interleave.

**Tech Stack:** unchanged. No new crates.

**Spec:** the findings restated in each task; `docs/superpowers/plans/2026-11-04-m2b-proactive.md` Tasks 6–7; `README.md` "Daemon, schedules, and inbox"; `CLAUDE.md`.

## Global Constraints

- All M1/M2a/M2b constraints hold. Ignored tests run before a task is done: `CHROMIUM=$PWD/.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium cargo test --release -- --ignored --test-threads=1 --skip live_`.
- **Never run `live_*`.** No real API call, no real Keychain.
- `tests/acceptance_m2b.rs` and `tests/acceptance/*.rs` are Claude's independent checks. **Never edit them.** `cargo test --test acceptance_m2b` must pass unchanged after every task. If a change here contradicts one, stop and report BLOCKED with the test name.
- No test is weakened. A test is deleted only when the named acceptance test asserts the same thing.
- Tasks 1 and 2 delete lines on net. Report `git diff --stat` per commit.
- Scratch only in `target/codex/`.

## Review Focus

- **Pause requested on a running task that then stops for approval; user approves:** the task runs, it does not re-pause. Test `approval_clears_pause_request` (Task 0).
- **Pause lands after the model chose an action but before it runs:** after `resume` the model is asked again on a fresh observation; the stale action never runs. Test `pause_drops_unexecuted_model_step` (Task 0).
- **Config has a search provider the extension can't use (`search?q={q}`):** the daemon still runs tasks, without the extension, and logs why. Test `daemon_runs_without_extension_on_bad_provider` (Task 0).
- **`grant x.example --schedule 99` for a schedule that doesn't exist:** exit 2, no grant stored. Test in `grant_cli_validates_schedule` (Task 0).
- **Expired grants in `grant list`:** shown as `expired`, not silently listed as live. Same test.

---

### Task 0: Correctness fixes

**Files:** `src/ledger.rs`, `src/engine.rs`, `src/daemon.rs`, `src/main.rs`; tests in `src/ledger.rs` (unit), `tests/engine.rs`, `tests/daemon.rs`, `tests/cli.rs`.

**Interfaces:**
- Produces: `Ledger::drop_pending_step(&self, id: i64, cursor: usize) -> anyhow::Result<()>`: if `steps.len() == cursor + 1`, truncate `steps` to `cursor`; otherwise no-op. One `IMMEDIATE` transaction, same pattern as `push_step`.

- [ ] **Step 1: Failing tests:**
  - Unit `approval_clears_pause_request` (ledger): `add` → `Executing` → `request_pause` → `transition(AwaitingHandoff(Approval))` → `approve` → `transition(Executing)` → `pause_if_requested(id) == false`. Same with `AwaitingHandoff(Challenge(_))` → `resume`.
  - Unit `drop_pending_step_only_drops_the_last_unrun_step`: task with 2 steps, cursor 1 → `drop_pending_step(id, 1)` → 1 step left. Call again with cursor 1 → still 1 step (no-op). Cursor 0 on a 2-step task → no-op.
  - `#[ignore]` `engine::pause_drops_unexecuted_model_step`: a mock model returns `click` ref 1; the test calls `request_pause` from the mock's request hook (after the reply is built, before the engine executes it). Result: state `AwaitingHandoff(Paused)`, `steps.len() == cursor`, the fixture server logged no click effect. `resume` + run with a mock `done` → `Committed`, and the mock saw 2 model requests.
  - `#[ignore]` `daemon::daemon_runs_without_extension_on_bad_provider`: config with the active provider `url_template = "search?q={q}"`, a queued scripted `navigate` task to the fixture server, `daemon::run` with `drain: true` → task `Committed`; stderr contains `extension disabled:`.
  - `tests/cli.rs` `grant_cli_validates_schedule`:
    - `grant x.example --schedule 99` with no schedules → exit 2, `grant list` empty.
    - After `schedule add 'every 1h' g` (id 1), `grant x.example --schedule 1` → exit 0.
    - A grant whose `expires_ms` is set in the past via rusqlite → `grant list` line contains `expired`.
- [ ] **Step 2:** run them → FAIL.
- [ ] **Step 3: Implement.**
  - `ledger.rs` `transition_from`: clear `pause_requested` when `to` is any `AwaitingHandoff(_)` or terminal (today: terminal or `Paused` only). Stopping for a handoff already honours the pause.
  - `engine.rs` "paused before step" branch (current line ~682): when `model_step.request.is_some()`, call `l.drop_pending_step(id, n)` after recording the step. Scripted steps are not dropped.
  - `daemon.rs` `launch`: `extension::install(...)` failure → `eprintln!("extension disabled: {error:#}")` and launch with `None`. Do not fail the launch.
  - `main.rs` grant add: with `--schedule <id>`, `ensure` the id is in `ledger.schedules()` else usage exit 2. `grant list`: print `expired` instead of `expires <time>` when `expires_ms <= now_ms()`.
  - `engine.rs` comments, one line each: at the grant `Config::load` ("fresh policy; a changed fingerprint only denies, never allows"), and at `recent` ("resets per run; approval pauses between repeats restart the count").
- [ ] **Step 4:** new tests pass; `cargo test -- --skip live_`, `cargo test --test acceptance_m2b`, full ignored suite pass.
- [ ] **Step 5:** Commit `fix: pause clears on handoff and drops unrun model step; daemon survives extension errors; grant CLI checks`.

### Task 1: Delete duplicate docs and tests

**Files:** `CLAUDE.md`, `src/gate.rs`, `src/ledger.rs`, `src/config.rs`.

- [ ] **Step 1:** `CLAUDE.md`: delete the whole `## README` section (line 20 to end, ~140 lines). Replace with:
  ```
  User docs, CLI usage and the launchd example live in `README.md`. Keep them there only.
  ```
  Keep the test commands and the `live_*` warning above it. Check `README.md` already has every user-facing paragraph being deleted (`diff` the two texts); move any that README lacks.
- [ ] **Step 2:** `src/gate.rs` `grant_scope_is_enforced`: replace with a ≤ 10-line test holding only cases `tests/acceptance/m2b_task6.rs` lacks: empty host and `shop.example.evil` → `None`; a grant issued under a config that already blocks the domain → `None`.
- [ ] **Step 3:** `src/ledger.rs` `pause_queued_and_resume`: delete everything `a7_pause_queued_task_and_resume` covers. Keep only: `request_pause` on a `Suggested` task errors, and a second `request_pause` on a paused task errors.
- [ ] **Step 4:** `src/config.rs` `domain_matching_and_policy_fingerprint`: delete the `expected` recomputation (it restates the function body). Keep the domain cases and the `unrelated` (`retention_days`) assertion.
- [ ] **Step 5:** `cargo test -- --skip live_` and `cargo test --test acceptance_m2b` pass. Commit `chore: drop tests and docs duplicated by acceptance checks and README`.

### Task 2: Code cuts

**Files:** `src/ledger.rs`, `src/engine.rs`, `src/extension.rs`, `src/main.rs`, `src/daemon.rs`, `Engine::launch` callers in `tests/`.

Each bullet is behaviour-preserving; existing tests are the check.

- [ ] **Step 1: ledger.**
  - One private `fn tasks(&self, sql: &str, params: impl rusqlite::Params) -> anyhow::Result<Vec<Task>>` using `query_map(...).collect()`; `list`, `queue` and `get` use it (`get` = first or "task {id} not found").
  - Delete `fire_due`; rename `fire_due_with_expiry` to `fire_due` (returns `(Vec<i64>, Vec<i64>)`). Update callers and tests.
  - Delete `next_queued`; tests use `queued(1)?.into_iter().next()`.
  - Delete the `queued`/`queued_humans` wrappers; make `queue(limit, human_only)` `pub`. Update `daemon.rs`.
- [ ] **Step 2: engine.**
  - `Engine::launch(chromium, profile, mock_keychain, extension: Option<&Path>)`; delete `launch_with_extension`. Update every caller (`daemon.rs`, `tests/*`).
  - Set `step_decision` before the step executes and delete the `unwrap_or_else` fallback at the error-record site (~line 816).
  - Delete the two `pause_if_requested` checks inside the replay loop and the `Option` wrapper they need (`Ok(Some(s))`, `let Some(s) = result else`). The loop-top check catches the pause before the next step.
  - Stuck detector: replace the `VecDeque` with `last: Option<(String, blake3::Hash)>` and `repeats: u8` (`repeats = if last == Some(pair) { repeats + 1 } else { 1 }`; fail at 3; reset both where `recent.clear()` is today). Drop the `VecDeque` import.
- [ ] **Step 3: extension.** `manifest(cfg) -> anyhow::Result<Value>` (no `expect`). `install` builds the manifest first and fails if `chrome_settings_overrides.search_provider.favicon_url` is empty; delete its separate provider lookup and `search_origin` check.
- [ ] **Step 4: main.**
  - `grant_args` domain check becomes: non-empty, no empty label, bytes in `[A-Za-z0-9.-]`. Delete the label-length and hyphen-position rules.
  - `libc::isatty(0)` → `std::io::stdin().is_terminal()` (`use std::io::IsTerminal`).
- [ ] **Step 5: daemon.** `libc::flock(LOCK_EX|LOCK_NB)` → `file.try_lock()` (stable since 1.89; `rust-version` is 1.97). Map `TryLockError::WouldBlock` to the existing `another iksa run is active` error.
- [ ] **Step 6:** `cargo build --release` with no new warnings; `cargo test -- --skip live_`, `cargo test --test acceptance_m2b`, full ignored suite pass. `git diff --stat HEAD~1` shows net deletions.
- [ ] **Step 7:** Commit `refactor: shorter ledger queries, engine launch and stuck detector; std file lock and tty check`.

---

## Done checklist

| # | Task | Check | Pass |
|---|---|---|---|
| R1 | 0 | full ignored suite `--skip live_` | Approve after pause runs; stale model step dropped; bad provider still runs tasks; grant CLI checks |
| R2 | 1 | `wc -l CLAUDE.md` | ≤ 25 lines; README.md holds the user docs |
| R3 | all | `cargo test --test acceptance_m2b` | Passes, files untouched: `git diff dd6882f -- tests/acceptance/` is empty and `tests/acceptance_m2b.rs` differs only by the two removed `cfg` lines |
| R4 | 1–2 | `git diff --stat` per commit | Net deletions; target ≈ −300 lines across Tasks 1–2 |

After R4: resume M3 at Task 2.
