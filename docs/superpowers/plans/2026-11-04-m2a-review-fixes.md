# iksa — M2a Review Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Apply the 2026-11-04 caveman review (correctness) and ponytail review/audit (complexity) findings to the uncommitted M2a Task 7 eval harness and the core, so the paid live eval runs on a harness that cannot crash mid-spend or over-credit answers.

**Architecture:** No new features.
- Task 0 hardens the eval relay and grading and commits the harness.
- Task 1 removes dead and duplicate code from the harness.
- Task 2 removes the `thiserror` dependency and the duplicated scrub code from the core.

The live eval (M2a Task 7 Step 2) runs **after** this plan, unchanged in approval rules.

**Tech Stack:** M2a stack minus `thiserror`.

**Spec:** the review findings in this conversation, restated in each task; `docs/superpowers/plans/2026-11-03-m2a-planner.md` Task 7; `CLAUDE.md`.

## Global Constraints

- All M1/M2a Global Constraints hold. Ignored tests run before a task is done: `CHROMIUM=$PWD/.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium cargo test --release -- --ignored --test-threads=1 --skip live_`.
- **Never run `live_eval` or any `live_*` test.** No real API call and no real Keychain access.
- No test is weakened. Privacy assertions (`assert_private`, never_send empty requests) keep panicking: they are invariants, not grades.
- Every task deletes lines on net, except Task 0 (fixes).
- Scratch only in `target/codex/`. Tests never write under it.

## Review Focus

- **Malformed request reaching the relay during a live run:** the client gets a 400, the run continues, and `EVAL_RESULT` is still printed. Test `relay_rejects_malformed_request` (Task 0).
- **Wrong numeric answer containing the right digits (`119` for `19`):** graded as failed. Test `grading_numeric_answers_need_whole_number` (Task 0).
- **Upstream reply close to 60 s:** the relay gives up before the client does, so the step is retried rather than lost. Test `relay_timeout_below_client_timeout` (Task 0).
- **`tasks.json` reordered:** reference capture and run order are still right, because they are keyed by case id. Test `external_order_names_every_external_case` (Task 0).
- **Ledger errors after removing `thiserror`:** callers and the CLI print the same messages. Test `ledger_errors_keep_messages` (Task 2).

---

### Task 0: Eval harness correctness, then commit it

**Files:** `tests/eval/support.rs`, `tests/eval.rs`, `.gitignore`.

- [x] **Step 1: Failing tests (in `tests/eval.rs`, non-ignored):**
  - `relay_rejects_malformed_request`:
    - `Relay::mock()`; open a `TcpStream` to its endpoint and write `GET / HTTP/1.1\r\n\r\n`;
    - read the response; assert it starts with `HTTP/1.1 400`;
    - then `iksa::llm::call(&relay.config(), "offline-key", &json!({"messages":[]}))` still succeeds;
    - `relay.finish()` returns without panicking, with 2 events: the first `status == 400` and `request == Value::Null`.
  - `grading_numeric_answers_need_whole_number`: a case with `answer: Some("19")`, state `Committed`.
    - `"done: 19 credits"` → true;
    - `"done: 119 credits"` → false;
    - `"done: price 19."` → true.
    - Case `"09:40"`: `"done: departs 09:40"` → true and `"done: 109:40"` → false.
    - The text case `"NOVA-42"` keeps its current behaviour.
  - `relay_timeout_below_client_timeout`: `assert!(support::RELAY_UPSTREAM_TIMEOUT < iksa::llm::CLIENT_TIMEOUT)`.
  - `external_order_names_every_external_case`: `support::CAPTURE_ORDER` and `support::RUN_ORDER` are each a permutation of the ids of the 5 non-local cases. `RUN_ORDER` starts with `"hacker-news"`, `"duckduckgo"`; `CAPTURE_ORDER` ends with `"duckduckgo"`, `"hacker-news"`.
- [x] **Step 2:** `cargo test --test eval` → the 4 new tests FAIL (compile errors count).
- [x] **Step 3: Implement.**
  - `src/llm.rs`: `pub const CLIENT_TIMEOUT: Duration = Duration::from_secs(60);`, used where `timeout_global` is set now (line 56). The value is unchanged.
  - `support.rs`:
    - `pub const RELAY_UPSTREAM_TIMEOUT: Duration = Duration::from_secs(55);`, used at the Forward agent's `timeout_global`.
    - Relay request parsing (lines 276–294) returns a `Result` instead of `unwrap`/`assert!`/`panic!`. On any parse error (bad request line, missing `content-length`, length > 1 000 000, bad JSON), write `400` with body `{"error":{"type":"eval_relay","message":"<reason>"}}` and record an `Exchange { request: Value::Null, status: 400, measurement: Measurement::default(), .. }`. An `accept` error other than `WouldBlock` is treated the same way: skip and continue, no panic.
    - `grade`: when `expected` has no ASCII letters, a match requires that the characters immediately before and after the occurrence in the normalized answer are not ASCII digits. Text answers keep `contains`.
    - `pub const CAPTURE_ORDER: [&str; 5] = ["wikipedia", "example", "mdn", "duckduckgo", "hacker-news"]` and `pub const RUN_ORDER: [&str; 5] = ["hacker-news", "duckduckgo", "wikipedia", "example", "mdn"]`. Look up cases by `id`, replacing the index arrays at lines 692 and 712. Local cases still run after them, in manifest order.
    - Line 785: bind `ledger.get(id)?` once.
  - `.gitignore`: add `/reports` and `/research_notes`.
- [x] **Step 4:** `cargo test -- --skip live_` passes, plus the full ignored suite with `--skip live_` (it includes `offline_eval_harness`).
- [x] **Step 5:** Commit everything uncommitted from Task 7 prep plus these fixes. `git status` must show `reports/` and `research_notes/` ignored and not staged.
  ```bash
  git add .gitignore src/llm.rs tests/eval.rs tests/eval docs/perf.md
  git commit -m "test: M2a eval harness (live run pending), relay and grading hardened"
  ```

### Task 1: Eval harness trims

**Files:** `tests/eval/support.rs`, `tests/eval.rs`.

Each item is a pure deletion or shrink. Behaviour and all other tests are unchanged.

- [x] **Step 1: Delete:**
  - the 1-hour cache-write pricing in `measure` (`hour` variable and `hour * 8.0`). iksa sends only 5-minute ephemeral cache, so price `writes * 5.0`;
  - `estimate()`, its test `estimate_states_its_token_assumptions` and the `println!("{}", estimate())` in `run_isolated` (the estimate lives in `docs/perf.md`);
  - test `external_eval_requires_write_approval` and, if then unused outside `run_isolated`, inline `case_risk`.
- [x] **Step 2: Shrink:**
  - **Totals:** replace `aggregate(&[Exchange])` and the three `try_fold` totals at the end of `run_isolated` with one `fn total<'a>(m: impl IntoIterator<Item = &'a Measurement>) -> Measurement`, used for per-row and overall totals.
  - **Relay accept loop:** a blocking `accept()`, with no `set_nonblocking`, no `WouldBlock` arm and no 5 ms sleep. `shutdown` sets `stop`, then makes one `TcpStream::connect(endpoint)` to wake `accept`. The loop checks `stop` after each accept and drops that wake-up connection without recording it.
  - **Rows:** `Vec<Option<Row>>` sized to the case count, filled by index. Delete the `(usize, Row)` tuples and the sort.
- [x] **Step 3:** `cargo test --test eval` + `offline_eval_harness` (ignored, `--skip live_`) pass. `relay_drains_late_exchange_before_reporting` and `relay_preserves_body_failure_retries` still pass unchanged.
- [x] **Step 4:** `git diff --stat` shows net deletions in both files. Commit `refactor: trim eval harness`.

### Task 2: Core trims (`thiserror`, scrub helpers, lib.rs)

**Files:** `src/ledger.rs`, `src/engine.rs`, `src/lib.rs`, `Cargo.toml`, `docs/superpowers/plans/2026-11-04-m2b-proactive.md`.

**Interfaces:**
- Produces: every `Ledger` method returns `anyhow::Result<_>`. Errors carry exactly the current `Display` texts: `task {id} not found`, `illegal transition from {from:?} to {to:?}`, and the transparent SQL/JSON errors via `?`.

- [x] **Step 1: Failing test:** `ledger::ledger_errors_keep_messages`:
  - `get(-1)` error `to_string() == "task -1 not found"`;
  - a stale-handle transition error `to_string() == "illegal transition from Executing to Executing"`.

  Change the two tests that match on variants (`stale_handle_transition_rejected`, the `NotFound(-1)` loop) to assert on `to_string()` with the same texts. Run it: it FAILS to compile until Step 2 lands, because `LedgerError` still exists and the new helper type doesn't.
- [x] **Step 2: Implement:**
  - Delete `LedgerError` and the `thiserror` dependency; use `anyhow::bail!`/`anyhow!` with the texts above.
  - Replace `scrub_snapshot` and `scrub_model` in `engine.rs` with `fn scrub_forms(text: &str, secrets: &[String], forms: impl Fn(&str) -> Vec<String>) -> String`, which does the shared work (skip empty secrets, collect forms, sort longest first, dedup, `scrub`). Each call site passes its own encoding closure: the HTML variants, or raw + JSON-escaped. `snapshot_scrubs_html_escaped_secrets` must pass unchanged.
  - `lib.rs`: one sorted block of `pub mod` lines; keep `#[doc(hidden)]` on `testutil`.
  - In the M2b plan, replace every `Result<…, LedgerError>` with `anyhow::Result<…>` and `→ Illegal` with `→ Err("illegal transition …")`.
- [x] **Step 3:** `cargo test -- --skip live_` + full ignored suite `--skip live_` pass. `cargo tree | grep thiserror` prints nothing for the `iksa` crate's direct deps (a transitive one is fine). `ls -l target/release/iksa` is recorded.
- [x] **Step 4:** Commit `refactor: drop thiserror, share secret-scrub forms`.

---

## Done checklist

| # | Task | Check | Pass |
|---|---|---|---|
| F1 | 0 | 4 new eval tests + `offline_eval_harness` | Pass; harness committed; `reports/`, `research_notes/` ignored |
| F2 | 1 | `git diff --stat HEAD~1` | Net deletions; eval tests unchanged and passing |
| F3 | 2 | full suites `--skip live_`; `cargo tree -e normal --depth 1` | Pass; no `thiserror` |

Done: Tasks 0–2 implemented in commits `67566e5`, `d414ff5`, `903524c`.

After F3: the user runs the live eval with the M2a Task 7 command, with `ANTHROPIC_API_KEY` exported in their own terminal. Codex then fills in D9 from `target/codex/task7-live.log` and makes the Task 7 results commit.
