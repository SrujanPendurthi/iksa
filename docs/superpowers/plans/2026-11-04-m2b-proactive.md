# iksa — M2b Proactive Agents Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** iksa acts without being asked each time, and asks for human attention as rarely and as precisely as safety allows.
- `iksa daemon` fires scheduled goals (interval or cron) and runs queued work with up to `max_agents` tasks at once.
- It keeps a suggestion inbox the user accepts or dismisses.
- Standing grants let routine Write steps proceed without a pause.
- A task can be paused while it runs, and a looping task fails on its own.

**Architecture:**
- **Ledger:** gains the `Suggested`/`Dismissed` states and a `schedules` table.
- **`schedule.rs`:** pure module that parses specs and computes the next fire time.
- **`daemon.rs`:** one dispatcher loop on a tokio `LocalSet`. Each tick it moves due schedules into the ledger, then starts queued tasks on a shared `Rc<Engine>`; each worker opens its own `Ledger`/`Cas` connection. Chromium is launched lazily and closed after an idle period, so an idle daemon holds no browser.
- **`iksa run`:** becomes the daemon in drain mode: fire due schedules, run until nothing is queued or running, then exit.
- **Suggestion sources:** schedules in `suggest` mode (the default), and the planner's `done` tool proposing follow-up goals.

**Tech Stack:** M2a stack. tokio gains the `signal` and `rt` (LocalSet) features. `libc::localtime_r`/`mktime` for local time. No new crates.

**Spec:** `PRD.md` §1.1 pillar 3 (schedules, propose tasks) and pillar 2 (bounded concurrency), §3.2 (lifecycle states, schedule trigger, suggestion inbox), §3.7 (`max_agents`), §5 (concurrency, memory, egress). M2a plan: `docs/superpowers/plans/2026-11-03-m2a-planner.md`. User decisions (2026-11-03): M2 triggers are **schedule + suggestion inbox**. Watch triggers were not chosen. Page-visit triggers need human tabs, so they belong to M3.

**Research input:** `reports/DeepSeek Super Agent for iksa.md` (local, untracked). It covers the design of DeepSeek's dsh-superagent prototype. No code is copied (TypeScript/DSH runtime, no browser layer); the ideas below are re-implemented:
- observe-first proactivity → schedules default to `suggest`;
- fire-once-after-gap → missed runs collapse;
- revalidated standing grants;
- a slot reserved for human-started work;
- a "waiting on" view and a deterministic stuck detector;
- owner pause/resume.

## Decisions this plan makes

- **Schedules fire goals only.** Scripted JSON tasks stay manual (`add`). Schedule commands require a configured model, like `ask` (PRD §3.2: proactive features stay off until a model is configured).
- **Spec syntax** is one argument, either `every <N>m|h|d` (minimum 5 minutes) or a 5-field cron `min hour dom month dow` in **local time**:
  - Fields accept `*`, `N`, `a-b`, `*/s`, `a-b/s` and comma lists.
  - `dow` runs 0–7, where both 0 and 7 mean Sunday.
  - If both `dom` and `dow` are restricted, a day matches when either one does (Vixie cron).
- **Observe first:** a new schedule defaults to mode `suggest`, so fires land in the inbox. `schedule set <id> auto` promotes it once its record looks right: `schedule list` shows how many of its suggestions were accepted and how many dismissed. Limits: 32 schedules, minimum interval 5 minutes.
- **Standing grants:** `grant <domain> [--schedule <id>] [--days N]` (N defaults to 7, maximum 30).
  - A grant lets **Write** steps on that domain and its subdomains run without approval, either for one schedule's tasks or for any task.
  - Each use is revalidated against the expiry, `never_send`, and a policy fingerprint (BLAKE3 of `auto_max` + `never_send` at grant time). If `config.json` changed those, every grant goes inert until it is re-granted.
  - **Financial steps never use a grant** (PRD §3.3), and granting `financial` is impossible.
  - Each auto-run step's record names the grant (`decision: "grant:<id>"`).
- **Human work first:** tasks the user started (`ask`, `add`, `accept`, `approve`/`resume`) may use one slot beyond `max_agents`; scheduled `auto` tasks may not.
- **Pause:** `pause <id>` on a `Queued` or `Executing` task stops it before its next step, as `AwaitingHandoff(Paused)`; `resume <id>` continues it.
- **Stuck detector (no model):** a goal task whose model picks the same action (op + selector + text) 3 times in a row with an unchanged observation hash ends `Failed("stuck: repeated <op> 3 times")`. There is no wall-clock deadline: the step budget × `STEP_TIMEOUT` already bounds a run.
- **Missed runs collapse.** After sleep or downtime, a due schedule fires once, and its next fire time is computed from *now*.
- **No pile-up.** If the task the schedule created last time is still open (`Suggested`, `Queued`, `Executing` or `AwaitingHandoff`), the fire is skipped and only `next_ms` advances.
- **Suggestions never run by themselves.** `Suggested` → `Queued` happens only through `accept`. Dismissed tasks are terminal and garbage-collected like `Committed`.
- **Follow-up suggestions from the planner:** the `done` tool gains a required `suggestions` array.
  - At most 3 are kept, each trimmed, non-empty and ≤ 300 chars.
  - Each is scrubbed with the task's redactions before it is stored.
  - They are stored only on `Committed`, and deduplicated against open tasks with the same goal.
- **One process runs tasks.** `daemon` and `run` take the same `run.lock`. Every other CLI command only writes the ledger, and the daemon notices within one poll (2 s).
- **Idle daemon = no Chromium.** The browser closes after `idle_close` (60 s) with no running task and relaunches on demand. Idle footprint is iksa alone.
- **One digest notification, not one per event:** after a tick in which the set of tasks waiting on the user changed, the daemon runs `osascript -e 'display notification "<msg>" with title "iksa"'` once.
  - `<msg>` is counts only, e.g. `waiting on you: 2 approvals, 1 challenge, 3 suggestions`. Goal text never appears on the lock screen.
  - `IKSA_NOTIFY=off` disables it, which all tests set.
- **"Waiting on" in `list`:** each `AwaitingHandoff` row ends with what it needs:
  - `approve step N: <op> (<risk>)`;
  - `solve <kind> challenge`;
  - `paused`;
  - `blocked: <host> is on never_send`.
- **No `daemon install`.** README documents a `launchd` plist for start-at-login. Add a command when someone asks.

## Global Constraints

- Everything in the M1 and M2a Global Constraints still holds: binary < 60 MB, 0700 data dir, launch args, no TCP port, ignored tests run before a task is done, no `live_*` without the user's go-ahead, tests never write under `target/codex`.
- `max_agents`: config field, default **3**, valid **1..=8** (validation error names the field). PRD §5: at most `max_agents` agent contexts at once.
- The only new outbound traffic is none: schedules and the inbox are local. An idle daemon makes **zero** network connections and has **no** Chromium child.
- Poll interval `POLL = 2 s`; browser idle close `IDLE_CLOSE = 60 s`. Both are `daemon::Opts` fields, so tests shorten them; neither is a config setting.
- Existing CLI output stays byte-compatible: `run` prints `"{id}\t{state:?}"` per finished task plus the same `hint:` lines, and `no queued tasks` when nothing is due.
- `audit::gc` refuses to run while a task is `Executing`. The daemon calls it only when no worker is running (at start and at most hourly while idle).

## Review Focus

- **Laptop asleep for three days with an hourly schedule:** one task on wake, not 72. Test `missed_runs_collapse` (Task 2).
- **Yesterday's scheduled task still awaits approval:** no second copy is created. Test `open_task_skips_fire` (Task 2).
- **Second daemon, or `run` while the daemon runs:** refused with `another iksa run is active`, and the daemon is untouched. Test `daemon_holds_run_lock` (Task 3).
- **Daemon killed mid-task:** on restart the task is `Queued` again and runs. Test `daemon_recovers_executing` (Task 3).
- **Cron that never fires (`0 0 30 2 *`) or a malformed spec:** rejected at `schedule add` with the reason; nothing is stored. Tests `cron_never_fires_rejected`, `bad_spec_is_usage_error` (Tasks 1, 2).
- **A grant used where it doesn't apply:** other domain, expired, Financial step, `never_send` added later, or `auto_max` changed. Each must still pause. Test `grant_scope_is_enforced` (Task 6).

## Running this plan with Codex (gpt-6.1-sol)

Same as M2a. Codex does one task per run with TDD, runs all ignored tests (`--skip live_`), keeps scratch in `target/codex/`, never fakes a result, and stops after the commit to print that task's checklist rows. Claude reviews each commit before the next task. **Prerequisite:** M2a Task 7 is committed. If D9 lands below 16/20, M2a-tune comes first; M2b does not depend on the planner's score, only on its interfaces.

---

### Task 0: Suggested and Dismissed states, inbox CLI

**Files:** `src/ledger.rs`, `src/audit.rs` (gc), `src/main.rs`, `tests/cli.rs`.

**Interfaces:**
- Produces:
  - `State::Suggested`, `State::Dismissed`, added as unit variants so old rows still parse.
  - `Ledger::suggest(&self, goal: &str) -> anyhow::Result<Option<i64>>`: inserts a `Suggested` goal task with title = goal. Returns `None` without inserting if an open task (`Suggested | Queued | Executing | AwaitingHandoff(_)`) has the same goal.
  - `Ledger::accept(id) -> anyhow::Result<()>`: conditional `Suggested`→`Queued`.
  - `Ledger::dismiss(id) -> anyhow::Result<()>`: `Suggested`→`Dismissed`, sets `finished_ms`.
  - `Ledger::is_open(state: &State) -> bool`: a free fn `ledger::is_open`.
- CLI:
  - `inbox` lists `Suggested` tasks as `{id}\t{goal}`, or prints `inbox empty`.
  - `accept <id>` requires a configured model, with the same error as `ask`.
  - `dismiss <id>`.

- [ ] **Step 1: Failing tests:**
  - `ledger::suggest_accept_dismiss`:
    - suggest → state `Suggested`, `next_queued()` is `None`;
    - accept → `Queued`;
    - a second suggest → dismiss → `Dismissed` with `finished_ms.is_some()`;
    - accept on a dismissed task → `Err("illegal transition …")`; dismiss on a `Queued` task → `Err("illegal transition …")`.
  - `ledger::suggest_dedupes_open_goal`: suggest "x" twice → `Some`, then `None`. After the first is dismissed, suggest "x" → `Some`. Also `add_goal("x","x")` Queued, then suggest "x" → `None`.
  - `audit::dismissed_is_gc_eligible`: a dismissed task older than retention is removed by `gc`.
  - `cli::inbox_accept_dismiss`: the test inserts suggestions via `Ledger::open(home/ledger.db)` + `suggest`. `inbox` prints both. `accept 1` with no model → exit 1 with `configure "model"`. With a model, `accept 1` → `list` shows `Queued`. `dismiss 2` → `inbox empty`. `accept abc` → exit 2.
- [ ] **Step 2:** FAIL. **Step 3:** implement. Extend `transition`'s allowlist only for these two edges and reuse `transition_from`. Add the commands to `USAGE`.
- [ ] **Step 4:** `cargo test` + full ignored suite pass. **Step 5:** Commit `feat: suggestion inbox (Suggested/Dismissed)`.

### Task 1: Schedule specs

**Files:** Create `src/schedule.rs`; `src/lib.rs`.

**Interfaces:**
- Produces:
  - `pub enum Spec { Every(Duration), Cron(Cron) }`.
  - `pub fn parse(s: &str) -> Result<Spec, String>`.
  - `pub fn next_fire_ms(spec: &Spec, after_ms: u64) -> Option<u64>`: the smallest minute-aligned instant strictly after `after_ms` whose local time matches, or `after_ms + interval` for `Every`; `None` if none within 4 years.
  - `pub fn local(ms: u64) -> Civil` with `Civil { year: i32, month: u8, day: u8, hour: u8, minute: u8, weekday: u8 /*0=Sun*/ }`.
  - `Cron::matches(&self, c: &Civil) -> bool`.
  - `parse` rejects a cron whose `next_fire_ms(now)` is `None`, with `never fires`.

- [ ] **Step 1: Failing unit tests (in-module):**
  - `parse_interval`: `every 30m` → `Every(1800 s)`; `every 2h`, `every 1d`; `every 0m`, `every 5s`, `every m`, `sometimes` → `Err`.
  - `parse_cron_fields`: `30 9 * * 1-5` matches Mon 09:30, not Sat 09:30 or Mon 09:31. `*/15 * * * *` matches minute 45, not 46. `0 0 * * 7` matches Sunday. `0 0 1,15 * *` matches the 15th. Six fields, `60 * * * *` and `* * * 13 *` → `Err` naming the field.
  - `dom_or_dow`: `0 0 13 * 5` matches Fri the 14th and Wed the 13th, but not Thu the 12th.
  - `cron_never_fires_rejected`: `parse("0 0 30 2 *")` → `Err` containing `never fires`; `0 0 29 2 *` → `Ok`.
  - `next_fire_is_minimal` (TZ-independent): for `30 9 * * 1-5`, `0 * * * *` and `*/7 * * * *`, with `now = now_ms()`:
    - `t = next_fire_ms(&spec, now).unwrap()`;
    - assert `t > now`, `t % 60_000 == 0` and `cron.matches(&local(t))`;
    - for every minute `m` with `now < m < t` (step 60 000, at most 7 days), assert `!cron.matches(&local(m))`.
- [ ] **Step 2:** FAIL. **Step 3:** implement. Each field is a `u64` bitmask. `next_fire_ms` walks day by day over `local()` and checks month/dom/dow first, then hours and minutes within a matching day. Use `mktime` for local→epoch. Wall times that don't exist on a DST-forward day are skipped. On the repeated hour, the first match is used. `ponytail:` comment: linear day scan, at most 1 461 days.
- [ ] **Step 4:** pass. **Step 5:** Commit `feat: interval and cron schedule specs`.

### Task 2: Schedules in the ledger, `schedule` CLI

**Files:** `src/ledger.rs`, `src/main.rs`, `tests/cli.rs`.

**Interfaces:**
- Consumes: `schedule::{parse, next_fire_ms}`, `ledger::is_open`.
- Produces:
  - Table `schedules(id INTEGER PRIMARY KEY, spec TEXT NOT NULL, goal TEXT NOT NULL, auto INTEGER NOT NULL DEFAULT 0, next_ms INTEGER NOT NULL, last_task INTEGER)`, created `IF NOT EXISTS` in `open`.
  - `tasks.schedule_id INTEGER` column, added with the same `pragma_table_info` migration as `goal`; `Task.schedule_id: Option<i64>`.
  - `pub struct Schedule { id: i64, spec: String, goal: String, auto: bool, next_ms: u64, last_task: Option<i64>, accepted: u32, dismissed: u32 }`. The two counts are derived from the schedule's tasks: accepted = left `Suggested` for any state but `Dismissed`.
  - `Ledger::add_schedule(spec, goal, auto, next_ms) -> anyhow::Result<i64>`: refuses a 33rd schedule with `at most 32 schedules`.
  - `Ledger::set_schedule_auto(id, auto: bool) -> anyhow::Result<()>`.
  - `Ledger::schedules() -> anyhow::Result<Vec<Schedule>>`.
  - `Ledger::remove_schedule(id) -> anyhow::Result<()>` (`Err("task {id} not found")` if absent; created tasks are kept).
  - `Ledger::fire_due(now_ms: u64, next: impl Fn(&str, u64) -> u64) -> anyhow::Result<Vec<i64>>`:
    - one IMMEDIATE transaction;
    - for each schedule with `next_ms <= now_ms`, if `last_task` is missing or not open, insert a goal task with `schedule_id` set (`Queued` when `auto`, else `Suggested`, deduped like `suggest`) and set `last_task`;
    - always set `next_ms = next(spec, now_ms)`;
    - returns the new task IDs.
- CLI:
  - `schedule add [--auto] <spec> <goal…>` requires a model, parses the spec (an error is a usage error, exit 2, with the reason on stderr; intervals under 5 minutes are rejected) and prints `{id}\tnext {YYYY-MM-DD HH:MM} local`.
  - `schedule list` prints `{id}\t{spec}\t{next local}\t{"suggest"|"auto"}\t{accepted}/{accepted+dismissed} accepted\t{goal}`.
  - `schedule set <id> auto|suggest`.
  - `schedule rm <id>`.

- [ ] **Step 1: Failing tests:**
  - `ledger::fire_due_creates_and_advances`: two schedules due at 0, one `auto`. `fire_due(1000, |_, n| n + 60_000)` → 2 IDs, one `Queued` and one `Suggested`, both with `schedule_id` set and `next_ms == 61_000`. A second `fire_due(1000, …)` → empty.
  - `ledger::schedule_trial_counts`: three fires of a `suggest` schedule (the open task is closed each time). Accepting one and dismissing two gives `accepted == 1, dismissed == 2`.
  - `ledger::schedule_cap`: the 33rd `add_schedule` → `Err` containing `at most 32`.
  - `ledger::missed_runs_collapse`: `next_ms = 0`, `fire_due(72 * 3_600_000, …)` → exactly 1 task.
  - `ledger::open_task_skips_fire`: fire → task `Queued`. Moving `next_ms` back to 0 and firing again → no new task, `next_ms` advanced. Transition that task to `Failed` and fire again → new task.
  - `cli::schedule_add_list_rm`:
    - `schedule add "every 2h" check hn` with a model configured prints an id and `next `;
    - `schedule list` shows `every 2h`, `suggest` and `0/0 accepted`;
    - `schedule set 1 auto` → list shows `auto`;
    - `schedule add --auto "0 9 * * 1" weekly digest` shows `auto`;
    - `schedule rm 1` → list has 1 row; `schedule rm 9` → exit 1.
  - `cli::bad_spec_is_usage_error`: `schedule add "every 5s" x` → exit 2, stderr contains `every 5s`. `schedule add "every 4m" x` → exit 2. `schedule add "0 0 30 2 *" x` → exit 2 with `never fires`. No model → exit 1 `configure "model"`.
- [ ] **Step 2:** FAIL. **Step 3:** implement. Format local time via `schedule::local`.
- [ ] **Step 4:** pass. **Step 5:** Commit `feat: schedules (ledger + CLI)`.

### Task 3: Daemon (sequential), `run` as drain mode

**Files:** Create `src/daemon.rs`; `src/lib.rs`, `src/main.rs`, `src/engine.rs`, `Cargo.toml` (tokio `signal`, `rt`), `tests/cli.rs`, `tests/daemon.rs`.

**Interfaces:**
- Consumes: `Ledger::fire_due`, `schedule::next_fire_ms`, `Engine::launch/run`, `audit::gc`.
- Produces:
  - `pub struct Opts { pub drain: bool, pub poll: Duration, pub idle_close: Duration, pub chromium: Option<PathBuf> }`. `Default` is `drain: false`, 2 s, 60 s, `None` (= `setup::chromium_path(home)`, resolved only when a task needs the browser).
  - `pub async fn run(home: &Path, opts: Opts) -> anyhow::Result<()>`: takes the run lock (moved from `main.rs` into `pub fn lock(home) -> anyhow::Result<File>`), then `recover()`, `gc`, then the loop:
    - `fire_due` (with `next_fire_ms`; a `None` from it removes the schedule and prints `schedule {id} will never fire again; removed`);
    - run each queued task, printing exactly what `run` prints today;
    - send the digest notification when the waiting-on-you counts changed during the tick;
    - sleep `poll`, interrupted early by SIGINT/SIGTERM;
    - drop the engine after `idle_close` with nothing running.
  - With `drain`, the loop returns once nothing is queued. The first tick still prints `no queued tasks` and never launches Chromium when nothing is due.
  - On a signal it stops starting tasks, drops the engine and returns `Ok`. A task interrupted mid-run stays `Executing` and is recovered next start.
- CLI: `daemon` = `daemon::run(home, Opts::default())`. `run` = `daemon::run(home, Opts { drain: true, ..Default })`.
- `pub fn waiting_on(task: &Task) -> Option<String>` in `ledger.rs`: the "waiting on" text from Decisions, used by `list` as a final tab-separated column on `AwaitingHandoff` rows. `pub fn digest(tasks: &[Task]) -> Option<String>` in `daemon.rs` builds the count message; it returns `None` when nothing waits.

- [ ] **Step 1: Failing tests:**
  - All existing `tests/cli.rs` `run` tests pass unchanged. They are the regression suite for drain mode.
  - Unit `ledger::waiting_on_text`: an approval pause at cursor 2 on a Click with Write risk → `approve step 3: click (write)`. Challenge → `solve Turnstile challenge`. Committed → `None`.
  - Unit `daemon::digest_counts`: 2 approvals, 1 challenge and 3 suggested give `waiting on you: 2 approvals, 1 challenge, 3 suggestions`; no waiting tasks give `None`; singulars read `1 approval`.
  - `cli::daemon_fires_due_schedule`:
    - `schedule add "every 5m" x` (default `suggest` mode), then set `next_ms = 0` with `rusqlite` on `ledger.db`;
    - spawn `iksa daemon` (`CHROMIUM=/nonexistent`, `IKSA_NOTIFY=off`);
    - within 5 s `inbox` lists `x`;
    - SIGTERM → exit 0 within 3 s.
  - `cli::daemon_holds_run_lock`: while the daemon runs, `run` and a second `daemon` both exit 1 with `another iksa run is active`. The first daemon still answers SIGTERM with exit 0.
  - `daemon::daemon_recovers_executing` (`#[ignore]`): a goal task is forced to `Executing` in the DB. `daemon::run` with a mock model (the M2a `done` mock) and `drain: true` → task `Committed`.
  - `daemon::idle_daemon_closes_browser` (`#[ignore]`): one mock goal, `idle_close = 1 s`, `poll = 200 ms`. The test observes a Chromium child process while the task runs and none 3 s after `Committed`, while the daemon is still running. Then signal.
  - `daemon::daemon_runs_new_ask_without_restart` (`#[ignore]`): start the daemon with an empty queue, then `ledger.add_goal` from the test. `Committed` within 10 s.
- [ ] **Step 2:** FAIL. **Step 3:** implement. Chromium launch failure fails only the tasks that needed it (`Failed("launching Chromium …")`); the daemon keeps running.
- [ ] **Step 4:** `cargo test` + full ignored suite pass. **Step 5:** Commit `feat: iksa daemon; run drains via the daemon loop`.

### Task 4: `max_agents` concurrency

**Files:** `src/config.rs`, `src/engine.rs`, `src/daemon.rs`, `src/ledger.rs`, `tests/daemon.rs`.

**Interfaces:**
- Consumes: Task 3's loop.
- Produces:
  - `Config.max_agents: u32` (serde default 3, validate 1..=8).
  - `Ledger::queued(limit: usize) -> anyhow::Result<Vec<Task>>`, oldest first.
  - Engine: replace `self.browser.borrow_mut().take()` on cleanup failure with a `broken: Cell<bool>` flag. When set, new runs return `browser unavailable after cleanup timeout` and the daemon drops and relaunches the engine once no worker holds it. `RefCell<Option<Browser>>` stays for the drop.
  - The daemon runs inside a `tokio::task::LocalSet`. It keeps `running: HashSet<i64>` and starts queued tasks not in `running` with `spawn_local`. Each worker opens its own `Ledger` and `Cas` and shares `Rc<Engine>`. The dispatcher is single-threaded, so no claim race is possible.
  - Slot rule: a task with `schedule_id.is_some()` that never sat in `Suggested` is background. The inbox's accept counts as a human start, so `Ledger` records `accepted_ms` (new nullable column) on `accept`. Background tasks start only while running < `max_agents`; human tasks start while running < `max_agents + 1`.

- [ ] **Step 1: Failing tests (`#[ignore]`):** the mock model delays each reply by 1 s and records peak concurrent requests.
  - `daemon::max_agents_bounds_concurrency`: 4 one-step goals with `max_agents = 3` → peak 3, all `Committed`, wall < 3.5 s. With `max_agents = 1` → peak 1.
  - `daemon::human_task_gets_reserved_slot`: `max_agents = 1`, two `auto` schedule tasks plus one `ask` task, mock delay 1 s. Peak is 2, with the `ask` task among the first two started; the second scheduled task starts only after the first finishes.
  - `daemon::concurrent_tasks_isolated`: two goals on one fixture that sets a cookie and a `localStorage` value. Each task's extract sees only its own value.
  - Unit `config::max_agents_range`: 0 and 9 → `Err` naming `max_agents`; a missing field → 3.
- [ ] **Step 2:** FAIL. **Step 3:** implement.
- [ ] **Step 4:** full ignored suite, **including the M1 pillar tests**, passes. **Step 5:** Commit `feat: run up to max_agents tasks at once`.

### Task 5: Follow-up suggestions from the planner

**Files:** `src/planner.rs`, `src/engine.rs`, `tests/planner.rs` (or wherever the M2a mock-model browser tests live).

**Interfaces:**
- Produces:
  - The `done` tool schema becomes `{answer: string, suggestions: {type: array, items: string}}`, both required.
  - `SYSTEM` gains one sentence: `In done, suggestions may list up to three short follow-up goals the user might want next; use an empty list when none are useful.`
  - `pub fn suggestions(input: &Value) -> Vec<String>`: trimmed, non-empty, items over 300 chars dropped, deduplicated, first 3.
  - On `Committed` the engine stores each suggestion with `ledger.suggest(&scrub_model(s, &redactions))`. They reach the user through Task 3's digest notification, which counts `Suggested` tasks, so the engine reports nothing extra.
  - A `done` without `suggestions` stays valid (old mock replies); it means none.

- [ ] **Step 1: Failing tests:**
  - Unit `suggestions_are_bounded`: 5 items (one empty, one 301 chars, one duplicate) → the expected 3.
  - Unit `tools_shape`: `done.required == ["answer","suggestions"]`.
  - `#[ignore]` `done_suggestions_land_in_inbox`: the mock replies `done` with `["check prices weekly", "{{secret:demo}} value is S3CRET-VALUE"]`, with the fake-Keychain secret `demo` = `S3CRET-VALUE` used earlier in the goal. Result: 2 `Suggested` tasks, and no task title or goal contains `S3CRET-VALUE`.
  - `#[ignore]` `failed_task_suggests_nothing`: a step-budget failure creates no suggestions.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** pass. **Step 5:** Commit `feat: planner proposes follow-up goals to the inbox`.

### Task 6: Standing grants

**Files:** `src/ledger.rs`, `src/engine.rs`, `src/main.rs`, `tests/cli.rs`, `tests/engine.rs`.

**Interfaces:**
- Consumes: `gate::decide`, `Task.schedule_id`, the never_send suffix rule already in `engine.rs`. Move that rule into `pub fn config::domain_matches(host: &str, suffix: &str) -> bool` and reuse it for grants.
- Produces:
  - Table `grants(id INTEGER PRIMARY KEY, domain TEXT NOT NULL, schedule_id INTEGER, expires_ms INTEGER NOT NULL, policy TEXT NOT NULL)`.
  - `pub fn Config::policy_fingerprint(&self) -> String`: the BLAKE3 hex of `serde_json::to_string(&(&self.auto_max, &self.never_send))`.
  - `Ledger::add_grant(domain, schedule_id: Option<i64>, expires_ms, policy) -> anyhow::Result<i64>`.
  - `Ledger::grants() -> anyhow::Result<Vec<Grant>>`.
  - `Ledger::remove_grant(id) -> anyhow::Result<()>`.
  - `pub fn grant_for(grants: &[Grant], host: &str, schedule_id: Option<i64>, risk: Risk, now_ms: u64, cfg: &Config) -> Option<i64>`. It is a pure fn and returns `None` unless all of these hold:
    - `risk == Write`;
    - the host matches;
    - the schedule matches (a grant without `schedule_id` matches any task);
    - not expired;
    - `policy == cfg.policy_fingerprint()`;
    - the host is not on `never_send`.
  - Engine: where `decide` returns `NeedsApproval` for a non-approved step, it calls `grant_for` with the current page host. `Some(id)` runs the step and records `decision: "grant:<id>"`.
  - CLI:
    - `grant <domain> [--schedule <id>] [--days N]` (N 1..=30, default 7) prints `{id}\texpires {local}`;
    - `grant list`;
    - `grant rm <id>`;
    - a `--risk` flag does not exist, so no Financial grant can be made.

- [ ] **Step 1: Failing tests:**
  - Unit `grant_scope_is_enforced`: a base grant for `shop.example`, any schedule, unexpired, current fingerprint.
    - `Some` for Write on `shop.example` and `a.shop.example`.
    - `None` for:
      - `evilshop.example`;
      - Financial;
      - Read (Read never needs one);
      - expired;
      - a different `schedule_id` on a schedule-bound grant;
      - `never_send` now containing `shop.example`;
      - `auto_max` changed.
  - `#[ignore]` `engine::grant_runs_write_without_pause`: a scripted task on the local fixture (served at `127.0.0.1`, granted `127.0.0.1`) with Type then Click, both Write → `Committed` with no approval. Both records carry `decision == "grant:1"`. A Financial Click on the same host still pauses `AwaitingHandoff(Approval)`.
  - `cli::grant_add_list_rm`: `grant shop.example --days 3` prints `expires `. `grant list` shows it. `grant x --days 31` → exit 2. `grant rm 1` → empty list.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** full ignored suite passes. **Step 5:** Commit `feat: standing grants for routine write steps`.

### Task 7: Pause and stuck detector

**Files:** `src/ledger.rs`, `src/engine.rs`, `src/main.rs`, `tests/cli.rs`, `tests/planner.rs`.

**Interfaces:**
- Produces:
  - `Handoff::Paused`.
  - `Ledger::request_pause(id)`:
    - on a `Queued` task: transition directly to `AwaitingHandoff(Paused)` (new allowlisted edge);
    - on an `Executing` task: set a new nullable `pause_requested` column;
    - anything else → `Err("illegal transition …")`.
  - The engine checks `pause_requested` before each step. When set, it clears it and transitions to `AwaitingHandoff(Paused)`. A step already in flight finishes first.
  - `resume` accepts `Paused` as well as `Challenge`.
  - Stuck detector in the goal loop: keep the last 3 `(action JSON with selector, observation hash)` pairs. Three equal in a row → `Failed("stuck: repeated <op> 3 times")`.

- [ ] **Step 1: Failing tests:**
  - Unit `ledger::pause_queued_and_resume`: Queued → `request_pause` → `AwaitingHandoff(Paused)` → `resume` → `Queued`. `request_pause` on `Committed` → `Err("illegal transition …")`.
  - `#[ignore]` `planner::pause_stops_running_goal`: the mock model delays 1 s per turn. After the first request arrives the test calls `request_pause`. The task ends `AwaitingHandoff(Paused)` with fewer steps than the mock script. After `resume` and another run it reaches `Committed`.
  - `#[ignore]` `planner::stuck_model_fails`: the mock always answers `click` on the same ref of a static page → `Failed` containing `stuck: repeated click 3 times`, after exactly 3 click records.
  - `cli::pause_command`: `pause 1` on a queued task → `list` shows `paused` in the waiting-on column.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** full ignored suite passes. **Step 5:** Commit `feat: pause/resume running tasks; fail stuck model loops`.

### Task 8: Pillars and docs

**Files:** `tests/pillars.rs`, `docs/perf.md`, `CLAUDE.md` (README + module map).

- [ ] **Step 1: Pillar tests (`#[ignore]`):**
  - `idle_daemon_footprint`: start `iksa daemon` with no tasks for 10 s. No descendants (`common::descendants` is empty) and `tree_footprint_kib` ≤ 30 720 (30 MB), median of 3.
  - `idle_daemon_no_egress`: same setup, `lsof -nP -a -p <pid> -i` prints nothing (no sockets of any kind).
- [ ] **Step 2:** Run them plus the full ignored suite. Record in `docs/perf.md` under `## M2b`: idle daemon footprint, egress result, the `max_agents_bounds_concurrency` wall time and the binary size.
- [ ] **Step 3: CLAUDE.md.**
  - Module map: add `schedule` (interval/cron specs, local next-fire time) and `daemon` (dispatcher: schedules, inbox, bounded workers, lazy browser).
  - README: add `daemon`, `schedule add/list/set/rm`, `inbox`, `accept`, `dismiss`, `grant`, `pause`, and the `max_agents` field. State plainly that a grant lets Write steps on that domain run unattended until it expires, and that Financial steps always ask.
  - README: add a `~/Library/LaunchAgents/dev.iksa.daemon.plist` example (`ProgramArguments` = path to `iksa`, `daemon`; `RunAtLoad` true; `KeepAlive` true), loaded with `launchctl bootstrap gui/$(id -u) <plist>`. `ANTHROPIC_API_KEY` is not put in the plist; the Keychain key is used.
- [ ] **Step 4:** Commit `docs: M2b pillars, daemon README`.

---

## Done checklist

| # | Task | Check | Pass |
|---|---|---|---|
| E1 | 0 | `cargo test -- --skip live_` | Inbox ledger, gc and CLI tests pass |
| E2 | 1 | `cargo test --lib schedule::` | All 5 schedule tests pass |
| E3 | 2 | `cargo test -- --skip live_` | fire_due, missed-run, open-task and CLI tests pass |
| E4 | 3 | full ignored suite `--skip live_` | Old `run` tests unchanged; daemon lock, recover, idle-close pass |
| E5 | 4 | full ignored suite `--skip live_` | Peak concurrency == max_agents; isolation; M1 pillars still pass |
| E6 | 5 | full ignored suite `--skip live_` | Suggestions bounded, scrubbed, only on Committed |
| E7 | 6 | full ignored suite `--skip live_` | Grant scope enforced; Financial always pauses; records name the grant |
| E8 | 7 | full ignored suite `--skip live_` | Pause stops before next step; stuck loop fails after 3 |
| E9 | 8 | pillar tests | Idle daemon: no Chromium, ≤ 30 MB, no sockets |
| E10 | all | `ls -l target/release/iksa` | < 60 MB |

After E10: M3 plan (human browsing in iksa's profile, bundled MV3 extension UI with ledger/inbox/approvals over native messaging, Agent View, copy-on-write cookies, page-visit triggers, per-domain mutation lock, H2 address-bar check).
