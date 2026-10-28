# iksa — M1 Agent Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove the risky platform assumptions, then build the agent core: queue scripted tasks, run them in hidden isolated Chromium contexts behind risk gates, pause on CAPTCHA or approval, and emit a private, deduplicated CAS audit log plus Markdown report.

**Architecture:** Single Rust binary crate `iksa`. One headful, pinned ungoogled-chromium instance driven over CDP (`chromiumoxide`); each task runs in its own browser context with a `hidden` target. Pure-logic modules (CAS, snapshot chunking, config/gate/actions, ledger, challenge detection, audit) sit under a thin async `engine` and a std-args CLI. No LLM in M1: a task is a JSON list of actions, so behavior is deterministic and testable.

**Tech Stack:** Rust 1.97 (edition 2024), tokio, chromiumoxide 0.9, blake3 1.8, zstd, rusqlite 0.41 (`bundled`), serde/serde_json, thiserror 2, anyhow, futures.

**Spec:** `PRD.md` (currently `gemini-code-1793212240267.md`; Task 2 renames it). Pillars in PRD §1.1, decisions in §1.2. `CLAUDE.md` (currently `gemini-code-1793212267498.md`) is an empty stub; Task 2 fills it.

## Plan-level decisions (PRD silent)

- `Failed(reason)` is M1's extra terminal state; `Suggested`/`Dismissed` arrive with M2.
- Approval waits reuse `AwaitingHandoff` with reason `Approval` or `Challenge(kind)`.
- Settings live in `config.json` from day one; env vars only for test overrides (`IKSA_HOME`, `CHROMIUM`).

## Roadmap checklist (one plan per milestone; only M1 detailed here)

Every milestone ends with a **pillar check**: egress test green, process-tree memory measured and logged in `docs/perf.md`, every new setting reachable from `config.json`.

- [ ] **M1 Agent core** (this plan): spikes, CAS, snapshots, config/search/gates, CAPTCHA detection, ledger, audit + retention, CDP worker with privacy flags, CLI + `setup`/`update`. PRD 3.2 (ledger), 3.3, 3.4, 3.5, 3.7, 4.1 (detect + pause).
- [ ] **M2 Proactive agents**: `iksa daemon`; triggers (schedule, page visit via CDP target events, watch via snapshot-hash change); `Suggested`/`Dismissed` states + inbox; planner over a user-chosen model (cloud or local; Anthropic + OpenAI-compatible clients; tool-call/JSON-schema output validated into `Action`s; observe-act loop, one step at a time, not a whole plan up front); page = pruned accessibility tree with element refs, so `Click`/`Type` gain a `ref` target alongside `selector`; send guards (no form values, `never_send` domains, request bodies stored in CAS + linked from the audit report); API key in keychain; planner eval set (~20 real tasks, success rate tracked per model in `docs/perf.md`); `max_agents` concurrency. PRD 3.2.
- [ ] **M3 iksa extension UI**: bundled MV3 extension — side panel (ledger, inbox, approvals), Agent View (screencast + input forwarding), urgent hand-off alert, `chrome_settings_overrides.search_provider` generated from `config.json`; native-messaging host `iksa native-host` bridging to the daemon over a 0600 Unix socket. PRD 1.2.3, 4.1, stories 2 and 4.
- [ ] **M4 Session bridge + CoW**: cookie copy-in per agent context; per-domain mutation lock for agent write-steps on domains open in human tabs; sync back only cookies the agent changed, skipping any the human changed meanwhile. PRD 3.1, 4.2.
- [ ] **M5 Integrations**: MCP server (`rmcp`) over ledger/actions; research context (local ONNX embeddings in `spawn_blocking`, graph in SQLite); uBlock Origin + Bitwarden via `Extensions.loadUnpacked`. PRD 3.6.
- [ ] **v0.1.0-alpha gate**: every PRD §5 row measured and met.

## Global Constraints

- Binary footprint: < 60 MB (iksa core; Chromium downloaded separately).
- Agent action latency: < 25ms overhead per DOM interaction step (gate + snapshot + CAS write, excluding Chromium's own work).
- Storage overhead: < 500 KB per completed task.
- Memory, whole process tree: ≤ ~300 MB idle; ≤ ~1.2 GB for 5 tabs + 3 agents.
- Privacy: idle browser makes zero non-loopback connections; no telemetry; iksa's own network calls are only `setup`/`update` and user-configured model calls.
- Hashing: BLAKE3; blobs immutable once written.
- Risk levels: 0 = read-only, autonomous; 2 = financial, mandatory sign-off — no setting auto-approves level 2.
- Lifecycle: `Queued -> Executing -> AwaitingHandoff -> Committed` (+ `Failed`).
- Data dir: `$IKSA_HOME`, default `~/.iksa`, mode 0700 (holds `config.json`, `ledger.db`, `cas/`, `chromium/`, `profile/`).
- Chromium: pinned ungoogled-chromium in `$IKSA_HOME/chromium/`, SHA-256 verified; never the user's installed Chrome or profile.
- Dependencies limited to the Tech Stack list; anything else needs a reason in the commit message.
- `iksa/` gets its own git repo (it currently sits untracked inside the `Projects/Current` repo).

## Review Focus

- **Process killed mid-task**: task stuck `Executing`. Expected: next `iksa run` requeues it from its saved cursor. Test in Task 6.
- **Secret typed into a form** (`Type { secret: true }`): never in the audit log, report, or DB. Test in Task 7.
- **Selector never appears / page hangs**: expected 30s step timeout, `Failed("timeout at step N")`, report still renders. Test in Task 8.
- **`auto_max: 2` in config**: level-2 step still stops for approval. Test in Task 4.
- **Chromium not installed or fails to launch**: task stays `Queued`, CLI exits 1 with `error:` naming the fix (`iksa setup`). Test in Task 9.

---

### Task 1: Feasibility spikes (go/no-go before building on them)

Throwaway code in `spikes/` (not part of the crate). Output: `docs/spikes.md` with PASS/FAIL + evidence per item. Any FAIL stops the plan and goes back to the user with options.

**Files:** Create `spikes/` (scratch cargo project using chromiumoxide), `docs/spikes.md`.

- [x] **S1 ungoogled-chromium on this Mac**: download the latest macOS arm64 ungoogled-chromium release, verify SHA-256 against the release page, clear quarantine (`xattr -dr com.apple.quarantine`), launch with chromiumoxide (`disable_default_args()` + explicit args, `user_data_dir`). PASS = CDP connects, `Browser.getVersion` returns.
- [x] **S2 hidden targets**: `CreateTargetParams { hidden: Some(true), browser_context_id: Some(ctx), .. }` in a non-default context. PASS = navigation, `page.content()`, click/type work, and no window appears. Also record whether `Page.startScreencast` delivers frames from a hidden target; if not, record which way of showing it during hand-off works (`Target.activateTarget`, or a visible target in the same context).
- [x] **S3 egress**: launch with the Task 8 privacy args, idle 60s on `about:blank`, `lsof -nP -i` for the Chromium processes. PASS = only `127.0.0.1` / `::1`. List any remaining hosts and the flag that stops each.
- [x] **S4 extension plumbing**: load a 10-line unpacked MV3 extension with `chrome_settings_overrides.search_provider` (`https://duckduckgo.com/?q={searchTerms}`, `is_default: true`) and `nativeMessaging`; register a native host manifest (record which directory this Chromium + custom `user-data-dir` reads); ping-pong one message. PASS = address-bar search goes to DDG and the ping returns. Note any confirmation bubble.
- [x] **S5 memory**: idle headful instance + one hidden agent context on a mid-size page; sum RSS over the process tree. Record vs the 300 MB idle target.
- [x] **S6 bot visibility**: visit a Cloudflare-protected page (e.g. `https://nowsecure.nl`) in (a) a human tab with CDP attached only at browser level, (b) an agent context. Record whether each gets challenged. Sets expected hand-off load; no evasion work in M1.
- [x] **Step 7:** Commit `docs: feasibility spike results`.

### Task 1 findings requiring plan review (2026-10-28)

All six experiments completed: S1 PASS, S2 PARTIAL (Rust Page subcheck BLOCKED), S3 PASS, S4 PARTIAL (fresh-profile bubble observation BLOCKED), S5 FAIL, S6 PASS (observation). See `docs/spikes.md` for the six-row result table and evidence. Task 2+ has not
started. These changes are required before proceeding:

- Task 8 must account for chromiumoxide 0.9.1 accepting argument **keys without
  leading `--`**; passing the documented shell strings directly emits `----`.
  Keep a shell-form final list in configuration, but strip prefixes or build
  structured `Arg`s at the chromiumoxide boundary.
- `disable_default_args()` does **not** prevent chromiumoxide 0.9.1 from adding
  `--disable-extensions` when its extension list is empty. M3 must use
  `BrowserConfig::builder().extension(path)` (observed working).
- Chromium 156 hidden targets are CDP target type `other`. The protocol works,
  but chromiumoxide 0.9.1 `new_page(hidden)` times out and `get_page(id)` reports
  `Requested value not found.` Task 8 needs an explicit adapter/upstream change,
  dependency decision, or an agreed visible-target fallback before implementation. The Rust Page subcheck is BLOCKED with exact errors `deadline has elapsed` and `Requested value not found.`.
- Hidden animated targets produced zero screencast frames; a visible positive
  control produced frames. `activateTarget` does not create a window. M3 cannot
  promise a live hidden-target Agent View on this pin. A new visible target in
  the same browser context preserves cookies, but does not preserve the original
  DOM, JavaScript heap, entered form state or a challenge's page-local state.
- Throwaway macOS spike launches use `--use-mock-keychain` because the user
  observed a missing-keychain popup on every launch. This uses a fixed encryption
  key; production keychain handling remains a separate design decision.
- Task 5 rules match Turnstile widgets in both observed nowsecure.nl modes (top-level 200, no interstitial); do not equate widget presence with proven bypass or completed challenge.
- S4 runtime settings prove DuckDuckGo is the effective default and native ping-pong works; fresh-profile confirmation-bubble observation remains BLOCKED (`No on-screen scoped Chromium windows available`), so the aggregate is PARTIAL.
- Reconcile the measured idle RSS with the ~300 MB whole-tree target before
  Task 2+. Do not infer the unmeasured five-tab/three-agent budget from this spike.

### Task 2: Scaffold + CAS blob store

**Files:**
- Rename: `gemini-code-1793212240267.md` -> `PRD.md`, `gemini-code-1793212267498.md` -> `CLAUDE.md`
- Create: `Cargo.toml`, `src/main.rs` (`mod cas; fn main() {}` for now), `src/cas.rs`, `.gitignore` (`/target`, `/spikes/target`)
- Modify: `CLAUDE.md` — architecture paragraph (this plan's Architecture + PRD §1.1 pillars), module map, commands (`cargo test`; `CHROMIUM=<path> cargo test --release -- --ignored`), rule: unit tests in-module under `#[cfg(test)]`, Chromium tests `#[ignore]` in `tests/`, and **ignored tests must be run before a task is called done**.

**Interfaces — produces `cas::Cas`:**
- `Cas::open(root: &Path) -> io::Result<Cas>` — creates `root` with mode 0700
- `Cas::put(&self, bytes: &[u8]) -> io::Result<blake3::Hash>` — idempotent; zstd level 3; write `root/tmp-<random>` then `rename`
- `Cas::get(&self, h: &blake3::Hash) -> io::Result<Vec<u8>>` — `InvalidData` if content does not rehash to `h`
- `Cas::delete(&self, h: &blake3::Hash) -> io::Result<()>` (GC only)
- `Cas::all(&self) -> io::Result<Vec<blake3::Hash>>`
- `Cas::disk_bytes(&self) -> io::Result<u64>`
- Layout `root/ab/cdef…`.

- [ ] **Step 1:** `cd iksa && git init`, rename docs, `cargo init --name iksa`, add deps, fill `CLAUDE.md`.
- [ ] **Step 2: Failing tests in `src/cas.rs`:** `put_get_roundtrip` (hash == `blake3::hash(b"hello")`), `put_is_deduplicated` (second put of same 100 KB leaves `disk_bytes` unchanged), `corrupt_blob_detected` (`InvalidData`), `missing_blob_is_not_found` (`NotFound`), `root_is_owner_only` (mode & 0o777 == 0o700).
- [ ] **Step 3:** `cargo test cas` — FAIL.
- [ ] **Step 4:** Implement.
- [ ] **Step 5:** `cargo test cas` — 5 passed.
- [ ] **Step 6:** Commit `feat: CAS blob store with BLAKE3 + zstd`.

### Task 3: DOM snapshot chunking

**Files:** Create `src/snapshot.rs`.

**Interfaces:**
- `snapshot::chunks(html: &str) -> Vec<&str>` — concatenation equals input
- `snapshot::put(cas: &Cas, html: &str) -> io::Result<blake3::Hash>` — chunk blobs + tree blob `"tree\n"` + one hex hash per line
- `snapshot::get(cas: &Cas, tree: &blake3::Hash) -> io::Result<String>`
- `snapshot::chunk_hashes(cas: &Cas, tree: &blake3::Hash) -> io::Result<Vec<blake3::Hash>>`

Chunk rule: cut after a `>` byte when chunk ≥ 1024 bytes and `blake3::hash(last 16 bytes).as_bytes()[0] < 8`; force-cut at 16384 bytes.

- [ ] **Step 1: Failing tests** (fixture: 2,000 `<div class="row" id="r{i}">item {i} …</div>` rows ≈ 200 KB): `chunks_concat_to_input` (+ no chunk > 16384), `small_edit_shares_chunks` (≥ 90% shared after inserting `<b>x</b>` mid-page), `roundtrip`, `ten_step_task_under_500kb` (10 snapshots, one small edit each, `disk_bytes() < 500_000`), `put_overhead_under_25ms` (`#[ignore]`, `--release`).
- [ ] **Step 2:** `cargo test snapshot` — FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test snapshot` and `cargo test --release snapshot -- --ignored` pass.
- [ ] **Step 5:** Commit `feat: content-defined DOM snapshot chunking`.

### Task 4: Config, search providers, risk gate, actions

**Files:** Create `src/config.rs`, `src/gate.rs`, `src/action.rs`.

**Interfaces:**
- `gate.rs`: `enum Risk { Read = 0, Write = 1, Financial = 2 }` (serde lowercase, `Ord`); `enum Decision { Auto, NeedsApproval }`; `gate::decide(auto_max: Risk, r: Risk) -> Decision` — `Financial` always `NeedsApproval`.
- `config.rs`:
  - `struct SearchProvider { name: String, url_template: String /* contains "{q}" */ }`
  - `struct Config { search: String /* active provider name */, providers: Vec<SearchProvider>, auto_max: Risk, max_agents: usize, retention_days: u32 }`
  - `Default`: providers DuckDuckGo `https://duckduckgo.com/?q={q}`, Kagi `https://kagi.com/search?q={q}`, Brave `https://search.brave.com/search?q={q}`, SearXNG `https://searx.be/search?q={q}`; `search: "DuckDuckGo"`, `auto_max: Read`, `max_agents: 3`, `retention_days: 30`.
  - `Config::load(home: &Path) -> anyhow::Result<Config>` — missing file writes default; `deny_unknown_fields`; errors on active provider not in list, template without `{q}`, `max_agents == 0`.
  - `Config::search_url(&self, query: &str) -> String` — percent-encode everything outside `A-Za-z0-9-._~` (~10 lines, no crate).
- `action.rs` (`#[serde(tag = "op", rename_all = "snake_case")]`): `Navigate { url }`, `Search { query }`, `Click { selector, risk = Write }`, `Type { selector, text, secret = false, risk = Write }`, `Extract { selector }`. `Action::risk()` (Navigate/Search/Extract = `Read`), `Action::redacted()` (secret `Type` text -> `"••••"`).

- [ ] **Step 1: Failing tests:** `read_auto_write_needs_approval_by_default`; `financial_always_needs_approval` (`decide(Financial, Financial) == NeedsApproval`, `decide(Financial, Write) == Auto`); `default_config_written_and_reloaded`; `kagi_search_url` (active `Kagi`, `search_url("rust & c++") == "https://kagi.com/search?q=rust%20%26%20c%2B%2B"`); `custom_provider_without_q_rejected`; `unknown_active_provider_rejected`; `unknown_config_field_rejected`; `action_json_shape` (`{"op":"click","selector":"#buy","risk":"financial"}`; risk defaults to `Write`); `redacted_hides_secret`.
- [ ] **Step 2:** `cargo test config gate action` — FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Pass.
- [ ] **Step 5:** Commit `feat: config with search providers, risk gate, action model`.

### Task 5: CAPTCHA / challenge detection

**Files:** Create `src/challenge.rs`.

**Interfaces:** `enum ChallengeKind { CloudflareChallenge, Turnstile, HCaptcha, ReCaptcha }` (serde); `challenge::detect(status: Option<u16>, headers: &[(String, String)], html: &str) -> Option<ChallengeKind>`.

Rules, first match wins, header names case-insensitive:
1. header `cf-mitigated: challenge`, or status 403/503 + `server: cloudflare` + html contains `Just a moment` -> `CloudflareChallenge`
2. `challenges.cloudflare.com/turnstile` or `class="cf-turnstile"` -> `Turnstile`
3. `hcaptcha.com/` or `class="h-captcha"` -> `HCaptcha`
4. `google.com/recaptcha` or `class="g-recaptcha"` -> `ReCaptcha`

- [ ] **Step 1: Failing tests:** one per rule (`cf_header`, `cf_interstitial`, `turnstile_iframe`, `hcaptcha_div`, `recaptcha_script`), `plain_403_is_not_challenge`, `article_mentioning_captcha_is_not_challenge`.
- [ ] **Step 2:** `cargo test challenge` — FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Pass.
- [ ] **Step 5:** Commit `feat: CAPTCHA/anti-bot challenge detection`.

### Task 6: Task ledger (SQLite)

**Files:** Create `src/ledger.rs`.

**Interfaces:**
- Consumes `Action` (Task 4), `ChallengeKind` (Task 5).
- `enum Handoff { Approval, Challenge(ChallengeKind) }`; `enum State { Queued, Executing, AwaitingHandoff(Handoff), Committed, Failed(String) }` (stored as JSON text)
- `struct Task { id: i64, title: String, steps: Vec<Action>, cursor: usize, state: State, approved_step: Option<usize>, finished_ms: Option<u64> }`
- `enum LedgerError { Sql(#[from] rusqlite::Error), Json(#[from] serde_json::Error), NotFound(i64), Illegal { from: State, to: State } }` (thiserror)
- `Ledger::open(path)`, `add(title, &[Action]) -> i64`, `get(id) -> Task`, `list() -> Vec<Task>`, `set_cursor(id, usize)`, `next_queued() -> Option<Task>` (lowest id)
- `transition(id, to)` — legal: `Queued->Executing`; `Executing->{AwaitingHandoff, Committed, Failed, Queued}`; `AwaitingHandoff->{Queued, Failed}`; else `Illegal`. Sets `finished_ms` on `Committed`/`Failed`.
- `approve(id)` — only from `AwaitingHandoff(Approval)`; sets `approved_step = Some(cursor)`, `Queued`.
- `resume(id)` — only from `AwaitingHandoff(Challenge(_))`; `Queued`.
- `recover() -> usize` — all `Executing` -> `Queued`.

- [ ] **Step 1: Failing tests** (temp path under `std::env::temp_dir()`): `add_then_get`, `happy_path_transitions`, `committed_is_terminal`, `approve_only_from_approval_wait`, `recover_requeues_executing` (cursor preserved across reopen), `finished_ms_set_on_commit`.
- [ ] **Step 2:** `cargo test ledger` — FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Pass.
- [ ] **Step 5:** Commit `feat: SQLite task ledger with lifecycle state machine`.

### Task 7: Audit records, report, retention GC

**Files:** Create `src/audit.rs`; modify `src/ledger.rs` (`steps` table + methods).

**Interfaces:**
- `struct StepRecord { step: usize, action: Action, decision: String /* "auto"|"approved" */, after: Option<String> /* tree hash hex */, outcome: String, at_ms: u64 }` — a step's "before" is the previous record's `after`; never stored twice.
- `StepRecord::new(step, action: &Action, decision, after, outcome)` — stores `action.redacted()`; forces `after = None` when `action.risk() == Financial`.
- `Ledger::record_step(id, &StepRecord)`, `Ledger::records(id) -> Vec<StepRecord>`, `Ledger::delete_task(id)` (task + steps).
- `audit::render(task: &Task, recs: &[StepRecord], cas: &Cas) -> io::Result<String>` — `# Task {id}: {title}`, state line, per step `## Step N — {op}`, action JSON, decision, outcome (≤ 2,000 chars), `DOM: {changed}/{total} chunks changed` vs previous step's snapshot (`DOM: not stored` when `None`).
- `audit::gc(ledger: &Ledger, cas: &Cas, retention_days: u32, now_ms: u64) -> anyhow::Result<(usize, usize)>` — delete finished tasks older than retention, then mark-and-sweep blobs unreachable from remaining records; returns (tasks, blobs) removed.

- [ ] **Step 1: Failing tests:** `report_lists_every_step`, `secret_never_rendered` (neither record JSON nor report contains `hunter2`), `financial_step_stores_no_snapshot`, `records_roundtrip`, `dom_change_summary` (small edit -> not all chunks changed), `gc_removes_old_tasks_and_orphan_blobs` (old committed task gone, its unique blobs gone, blobs shared with a recent task kept, running task untouched).
- [ ] **Step 2:** `cargo test audit ledger` — FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Pass.
- [ ] **Step 5:** Commit `feat: audit records, Markdown report, retention GC`.

### Task 8: CDP engine worker

**Files:** Create `src/engine.rs`, `tests/engine.rs` (Chromium tests `#[ignore]`, need `CHROMIUM`).

**Interfaces:**
- `engine::chrome_args(profile: &Path) -> Vec<String>` — `--user-data-dir=<profile>`, `--remote-debugging-address=127.0.0.1`, `--disable-background-networking`, `--disable-component-update`, `--disable-sync`, `--no-pings`, `--disable-domain-reliability`, `--disable-field-trial-config`, `--disable-breakpad`, `--metrics-recording-only`, `--no-default-browser-check`, `--no-first-run`, `--safebrowsing-disable-auto-update`, plus whatever S3 found; never `--enable-automation`. `--use-mock-keychain` only in tests/CI (`headless == true`) — the real profile must use the macOS Keychain so cookies stay encrypted at rest (unit test asserts the flag is absent for headful launches). Launch with `disable_default_args()` so chromiumoxide defaults don't sneak in; profile dir 0700.
- `Engine::launch(chromium: &Path, home: &Path, headless: bool) -> anyhow::Result<Engine>` — headful by default; `headless` only for CI.
- `Engine::run(&self, ledger, cas, cfg: &Config, id) -> anyhow::Result<State>` and `Engine::run_with_timeout(.., step_timeout: Duration)`; `const STEP_TIMEOUT: Duration = 30s`.
- `// ponytail: CDP over loopback TCP port (chromiumoxide 0.9 has no pipe transport); move to --remote-debugging-pipe when available` at the launch site.

Algorithm for `run`:
1. `transition(Executing)`; new browser context; page via `CreateTargetParams { hidden: Some(true), browser_context_id, url: "about:blank" }` (or S2's fallback). If cursor > 0, re-navigate to the last `Navigate`/`Search` before cursor.
2. Per step from cursor: `gate::decide(cfg.auto_max, action.risk())`; `NeedsApproval` and `approved_step != Some(cursor)` -> `AwaitingHandoff(Approval)`, return.
3. Run action under timeout. `Search` = navigate to `cfg.search_url(query)`. Navigate/Search: status + headers from `wait_for_navigation_response()`. Click: `find_element(sel)?.click()`. Type: click then `type_str`. Extract: `inner_text()` -> outcome.
4. One snapshot after the action (`page.content()` -> `snapshot::put`; skipped for Financial); `record_step`; `challenge::detect` -> `Some(k)`: `AwaitingHandoff(Challenge(k))`, return without advancing cursor.
5. Error/timeout: record step with outcome `error: …` / `timeout at step N`; `Failed(..)`; return.
6. `set_cursor(cursor + 1)`; after last step `Committed`. Always dispose the context.

Fixture server in `tests/engine.rs`: ~20-line `std::net::TcpListener` thread; paths `/form` (`<h1>Form</h1><input id=q><button id=go>`), `/cf` (403 + `cf-mitigated: challenge`), `/slow` (never responds), `/search?q=…` (echoes decoded q in `<h1>`).

- [ ] **Step 1: Failing tests:**
  - unit (not ignored): `privacy_args_present_automation_absent`
  - `runs_read_task_to_committed` (Navigate `/form`, Extract `h1` == "Form")
  - `search_uses_configured_provider` (provider `Local` = `http://127.0.0.1:<port>/search?q={q}`; Search "a b", Extract `h1` == "a b")
  - `write_step_waits_for_approval` (-> `AwaitingHandoff(Approval)`, cursor 1; `approve`; run -> `Committed`)
  - `challenge_pauses` (`/cf` -> `AwaitingHandoff(Challenge(CloudflareChallenge))`)
  - `hung_page_fails_with_timeout` (`run_with_timeout` 2s on `/slow` -> `Failed` containing `timeout`; `render` succeeds)
  - `contexts_are_isolated` (cookie set by task A invisible to task B)
  - `idle_egress_is_loopback_only` (launch, idle 60s, `lsof -nP -i` over the process tree shows only loopback)
  - `step_overhead_under_25ms` (median over 20 Extract steps of (step wall time − time inside the CDP call) < 25ms, `--release`)
- [ ] **Step 2:** `cargo test engine` + `CHROMIUM=… cargo test --test engine -- --ignored` — FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Same commands — all pass.
- [ ] **Step 5:** Commit `feat: CDP engine worker with privacy args, hidden contexts, gates, challenge pause`.

### Task 9: CLI, setup/update, end-to-end, pillar check

**Files:** Modify `src/main.rs`; create `src/setup.rs`, `tests/cli.rs`, `examples/hn.json` (Search "hacker news", Navigate `https://news.ycombinator.com`, Extract `.titleline`).

**Interfaces:**
- `setup.rs`: `struct Pin { version, url, sha256 }`, one `const` per platform (`mac-arm64`, `mac-x64`, `linux-x64`; values from the release used in S1). `setup::chromium_path(home) -> Option<PathBuf>` (`$CHROMIUM` if set, else installed binary). `setup::install(home) -> anyhow::Result<PathBuf>` — `curl -fL`, verify with `shasum -a 256` (abort + delete on mismatch), unpack (`hdiutil attach -nobrowse` + `cp -R` + detach on macOS; `tar -xf` on Linux), `xattr -dr com.apple.quarantine`; idempotent. `iksa update` prints installed vs pinned and reinstalls if they differ (pin moves with iksa releases; no runtime version lookup).
- Commands (manual `std::env::args` match): `setup`, `update`, `add <task.json>` (`{"title","steps"}`; prints id), `run [--headless]` (`recover()`, `gc`, then each queued task), `list`, `approve <id>`, `resume <id>`, `report <id>`, `gc`. Bad usage: usage on stderr, exit 2. Error: `error: …` on stderr, exit 1.

- [ ] **Step 1: Failing tests** (`env!("CARGO_BIN_EXE_iksa")`, temp `IKSA_HOME`): `add_and_list`; `bad_task_json_exits_nonzero` (`{"op":"fly"}` -> exit 1, `error:`, ledger unchanged); `bad_config_exits_nonzero` (template without `{q}` -> exit 1, message names the provider); `chromium_launch_failure_keeps_task_queued` (`CHROMIUM=/nonexistent` -> exit 1, task still `Queued`); `run_without_setup_says_run_setup`; `checksum_mismatch_aborts_install` (unit test in `setup.rs`: local file + wrong hash -> `Err`, file removed); `e2e_report` (`#[ignore]`, network: add `examples/hn.json`, `run`, `report` contains `## Step 2` and `Committed`).
- [ ] **Step 2:** `cargo test --test cli` — FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** `cargo test && CHROMIUM=… cargo test --release -- --ignored` — all pass.
- [ ] **Step 5: Pillar check**, results into `docs/perf.md`: `ls -l target/release/iksa` < 60 MB; `du -sk $IKSA_HOME/cas` after `e2e_report` < 500; process-tree RSS idle and during `e2e_report`; egress test green.
- [ ] **Step 6:** Commit `feat: iksa CLI, pinned Chromium setup, end-to-end flow`.
