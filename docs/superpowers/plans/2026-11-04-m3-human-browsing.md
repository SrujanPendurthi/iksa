# iksa — M3 Human Browsing and Extension UI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The human browses in iksa's own Chromium. A bundled MV3 extension shows tasks, the inbox and approvals, and Agent View lets the human watch a task and solve a CAPTCHA in place. Agents reuse logins through copy-on-write cookies, page visits fire triggers, and a per-domain lock keeps agent writes off sites the human has open.

**Architecture:**
- **The daemon (M2b) stays the only process that drives Chromium.** It gains a Unix-domain control socket `IKSA_HOME/daemon.sock` (mode 0600: a filesystem object, not a network port).
- **The extension talks only to a native messaging host.** Chromium starts it as `iksa` with a `chrome-extension://…` argument; it relays length-prefixed JSON between stdio and that socket. The extension never makes network requests.
- **Extension files are embedded in the binary.** The daemon writes them to `IKSA_HOME/extension/` at each launch, with a manifest generated from `config.json` so the address-bar search provider follows the config.
- **Human windows are ordinary page targets** in the default browser context. Agents stay in hidden, isolated contexts.

**Tech Stack:** M2b stack. `std::os::unix::net` for the socket. Plain HTML/CSS/JS for the extension: no npm, no bundler, no framework.

**Spec:**
- `PRD.md` §1.2 (decisions 3 and 4), §3.1 (cookie sandbox), §3.2 (page-visit trigger), §3.5, §4.1 (Agent View hand-off), §4.2 (mutation lock), §5 (memory, egress).
- `docs/spikes.md`: S4 (native messaging manifest location, `search_provider` default) and V5/V6 (hidden-target screencast with immediate frame ack; input forwarding).
- `docs/perf.md` H2 (deferred address-bar check).
- M2b plan: `docs/superpowers/plans/2026-11-04-m2b-proactive.md`.

## User-confirmed choices (2026-11-04)

Reviewed and approved by the user. Codex must not change these without a new decision from the user.

1. **Cookie sharing is off until the user lists sites** in `share_cookies`.
   - The agent acts as the user only on listed sites, because logged-in pages carry account data to the model.
   - Changes the agent makes to cookies are copied back only when its task is `Committed`.
   - If the human changed the same cookie meanwhile, the human's value is kept. (Task 4)
2. **A CAPTCHA-paused task stays live for 15 minutes** (`HOLD`), so the user can solve it in Agent View and the agent continues on the same page. After that it falls back to replaying its steps from the start. (Task 5)
3. **The lock never applies to read-only steps**, and standing grants don't override it. Approving the step does. (Task 7)
4. **Memory is tight:** 5 tabs + 3 agents already measured 98–99.5% of 1.2 GB. If the extension pushes it over, stop and report BLOCKED with the numbers; never raise the limit. (Global Constraints, Task 8)
5. **The extension loading method is checked first.** `--load-extension` worked in spike S4. Task 0's first test confirms it on the pinned 156 build; if not, fall back to CDP `Extensions.loadUnpacked` with `--enable-unsafe-extension-debugging` and record which was used. (Task 0)

## Decisions this plan makes

- **Prerequisite:** M2b is complete (daemon, `schedule.rs`, `Handoff::Paused`, grants, `domain_matches`). M3 changes `daemon.rs`/`engine.rs` and runs no tasks of its own.
- **Socket, not port.** The PRD forbids an extra *listening port*; a 0600 Unix socket inside the 0700 data dir is reachable only by the owner. The pillar test still asserts zero TCP/UDP listeners. Protocol: one JSON object per line.
  - Request `{"id":n,"op":…}`, reply `{"id":n,"ok":…}` or `{"id":n,"error":"…"}`.
  - Requests are capped at 64 KiB and replies at 1 MiB, which is Chromium's host→extension limit.
- **Native host = the same binary.** If `argv[1]` starts with `chrome-extension://`, `main` runs `native_host::run()`.
  - The manifest goes to `IKSA_HOME/profile/NativeMessagingHosts/dev.iksa.host.json` (the S4-proven location), with `path` set to the absolute path of the current `iksa` executable.
  - `allowed_origins` lists exactly the iksa extension's ID.
- **Stable extension ID:** the manifest carries a fixed public `key` (a throwaway RSA public key committed to the repo; no private key exists), so the ID never changes. The extension has no `externally_connectable`, so web pages can't message it.
- **Extension loading:** `--load-extension=<IKSA_HOME/extension>` (proven by S4). Task 0's first test confirms it on the pinned 156 build. If the flag is ignored, use CDP `Extensions.loadUnpacked` with `--enable-unsafe-extension-debugging` and record which one was used in `docs/spikes.md`.
- **Search provider:** `chrome_settings_overrides.search_provider` is generated from the active provider in `config.json`, with `{q}` → `{searchTerms}`, `is_default: true`, `encoding: "UTF-8"`. Changing the provider takes effect at the next Chromium launch.
- **UI surfaces (two pages):**
  - `panel.html`, the side panel: tasks, inbox (accept/dismiss), and approve/resume/pause buttons. It polls `list` every 2 s while open.
  - `agent.html?task=<id>`: a screencast `<img>`, click/keys forwarded, and a **Resume agent** button.
  - No popup and no options page.
- **Agent View keeps the paused context alive.**
  - On `AwaitingHandoff(Challenge)` the engine no longer disposes the task's context. It holds it for `HOLD = 15 min`, keyed by task ID. `resume` then continues on the held target with **no replay**, so solved-challenge state survives.
  - Once the hold expires the context is disposed, and resume falls back to M1's replay.
  - Any task may be watched while `Executing`.
- **Screencast:** `Page.startScreencast {format:"jpeg", quality:60, maxWidth:1280, maxHeight:800, everyNthFrame:2}`, acknowledged immediately (V5). Frames over 1 MiB as base64 are dropped and counted. Input goes through `Input.dispatchMouseEvent` (press + release) and `Input.insertText`, plus `Input.dispatchKeyEvent` for Enter, Tab, Backspace and arrow keys.
- **Copy-on-write cookies, opt-in per domain.** New config field `share_cookies: Vec<String>` (default empty; matched with `config::domain_matches`). Default empty because logged-in pages carry account data to the model; the user chooses which sites the agent acts as them on.
  - At context creation, `Storage.getCookies` (default context) is filtered to shared domains, then `Storage.setCookies` copies them into the task context. The copied set is kept in memory as the *base*.
  - On `Committed` only, a three-way sync back: a cookie that is new or changed in the task context is written to the default context **only if** the human's current value still equals the base (the human wins conflicts). Deletions are not synced (a `ponytail:` comment notes it; add when logout flows need it).
  - Cookie values never enter the ledger, records, CAS or model requests. Pages may still show account data, which is the user's opt-in.
- **Page-visit triggers reuse schedules.** New spec form `visit <pattern>` in `schedule.rs`:
  - The pattern is `host[/path]`. `*` matches any characters; the host matches by `domain_matches`, and the path matches by prefix-glob against the URL path.
  - Modes, counts, `schedule set`, grants `--schedule` and the 32 cap all apply unchanged.
  - The goal may contain `{url}`, replaced with the visited URL without query or fragment.
  - Cooldown: 10 min per schedule (stored as `next_ms`).
  - Never fires for `never_send` hosts or non-HTTP(S) URLs, and the visited URL is stored only when a pattern matched.
- **Mutation lock (PRD §4.2):**
  - Before a **Write or Financial** step on host H, if a human page target's host H' satisfies `domain_matches(H, H') || domain_matches(H', H)`, the engine polls every 500 ms for up to `LOCK_WAIT = 30 s`.
  - If the tab is still open, the task pauses `AwaitingHandoff(Approval)` with outcome `waiting: <host> is open in your tab`. `approve` overrides the lock for that step.
  - Read steps are never blocked. Grants (M2b) do not bypass the lock.
- **Human browsing lifecycle:**
  - `iksa browse [url]` sends `{"op":"browse","url":…}` over the socket. The daemon launches Chromium if needed and opens `Target.createTarget {url, newWindow: true}` in the default context. Without a running daemon the command prints `start iksa daemon first`.
  - Idle-close (M2b) never fires while a human page target exists. The daemon tracks them with `Target.setDiscoverTargets` (type `page`; hidden agent targets report `other`).
- **Keychain:** the persistent human profile encrypts cookies, so the first real `browse` may show macOS's "Chromium Safe Storage" prompt. That is expected and documented; tests keep `--use-mock-keychain`.

## Global Constraints

- All M1, M2a and M2b constraints hold: binary < 60 MB; idle ≤ 300 MB; 5 tabs + 3 agents ≤ 1.2 GB (median of 3, summed `phys_footprint`); zero non-loopback egress at idle; no TCP port; 0700 data dir; ignored tests run with `--skip live_`; no `live_*` without the user.
- **No new crates.** No JS dependencies or build step; extension files are `include_str!`'d.
- **Extension:** MV3; permissions exactly `nativeMessaging`, `sidePanel`; no host permissions; no remote code; default CSP; no `fetch`/XHR/WebSocket anywhere in its JS. A grep check enforces this in Task 2.
- **Native host:** answers only its own origin (Chromium enforces `allowed_origins`); relays only to `IKSA_HOME/daemon.sock`; exits when stdin closes.
- **Memory watch:** C14 was already at 98–99.5% of 1.2 GB. Task 8 re-measures with the extension loaded and one human window. If it exceeds the budget, record it as BLOCKED with numbers. Do not raise the target.
- **C14 is a median of 3, as the constraint above says.** `tests/pillars.rs` `five_tabs_three_agents_under_1200mb` today asserts a single sample, which is stricter than the spec; live Wikipedia samples range 1,117,778–1,253,906 KiB on the same code (perf.md, `m2b-task3-ignored-memory-failure.log`, `m3-task0-*`, `m3-task1-ignored.log`). Change it to run 3 fresh launches, print every sample, and assert the **median** `< 1228800`. The limit stays 1,228,800 KiB. Record all three samples in `docs/perf.md`. If the median fails → BLOCKED with numbers. (Decided 2026-11-05 after M3 Task 1's single sample hit 1,244,644 KiB.)

## Review Focus

- **Side panel opened while the daemon is down:** the panel shows `iksa daemon is not running`, not a blank page. The host replies `{"error":"daemon not running"}` when the socket is missing. Test `host_reports_missing_daemon` (Task 1).
- **User closes Agent View without resuming, then resumes after the hold expired:** the task resumes by replay and does not error. Test `hold_expiry_falls_back_to_replay` (Task 5).
- **Human logs out or changes a cookie while the agent runs:** the human's value survives the sync. Test `human_cookie_change_wins` (Task 4).
- **Human tab navigates away from the locked site during the wait:** the lock clears within one poll and the step proceeds. Test `lock_clears_when_tab_leaves` (Task 7).
- **A web page or another extension trying to drive iksa:** it can't, because there is no `externally_connectable` and `allowed_origins` has exactly one origin. Test `manifests_restrict_origins` (Task 0).

## Running this plan with Codex (gpt-6.1-sol)

Same rules as M2b. **Order:** after Task 1 commits, run `docs/superpowers/plans/2026-11-05-m2b-review-fixes.md` (all 3 tasks), then continue at Task 2. From that plan on, `Engine::launch` takes `extension: Option<&Path>` (no `launch_with_extension`), `extension::manifest` returns `anyhow::Result<Value>`, and a failed `extension::install` makes the daemon launch without the extension. Browser tests are `#[ignore]` and use `--use-mock-keychain` with a temp profile; they never touch the real `~/.iksa` or the user's Chromium/Chrome data. Task 8's H2 step is **manual by the user**: Codex prepares it and stops.

---

### Task 0: Generated extension, native host manifest, launch

**Files:** Create `src/extension.rs`, `extension/manifest.base.json`, `extension/background.js`. Modify `src/engine.rs` (`launch_args`), `src/lib.rs`, `tests/m3.rs` (new).

**Interfaces:**
- Produces:
  - `pub const EXTENSION_ID: &str` (derived from the committed key; a unit test recomputes it as SHA-256 of the DER key, first 32 hex chars mapped `0-f` → `a-p`).
  - `pub fn install(home: &Path, cfg: &Config, exe: &Path) -> anyhow::Result<PathBuf>`: writes `IKSA_HOME/extension/*` and `IKSA_HOME/profile/NativeMessagingHosts/dev.iksa.host.json`, and returns the extension dir.
  - `pub fn manifest(cfg: &Config) -> serde_json::Value`.
  - `launch_args(profile, mock_keychain, extension: Option<&Path>)`: with `Some`, adds `--load-extension=<path>`.

- [x] **Step 1: Failing tests:**
  - Unit `manifest_follows_search_config`: Kagi active → `search_provider.search_url == "https://kagi.com/search?q={searchTerms}"`, `is_default == true`, `name == "Kagi"`. Permissions `== ["nativeMessaging","sidePanel"]`, and there is no `host_permissions` and no `externally_connectable`.
  - Unit `extension_id_matches_key`.
  - Unit `manifests_restrict_origins`: the host manifest's `allowed_origins == ["chrome-extension://<EXTENSION_ID>/"]`, `type == "stdio"`, `path` absolute.
  - Unit `launch_args_exact` (existing) is updated with the `None` case unchanged, plus a new `Some` case.
  - `#[ignore]` `m3::extension_loads_and_owns_search`: install into a temp home, then launch.
    - `Target.getTargets` contains a `service_worker` whose URL starts with `chrome-extension://<EXTENSION_ID>/`.
    - The S4 method reads `chrome://settings/searchEngines` (bounded 15 s shadow-DOM read) and contains the active provider name with `(Default)`.
- [x] **Step 2:** FAIL. **Step 3:** implement. Embed `extension/*` with `include_str!`. Commit the public key only.
- [x] **Step 4:** full ignored suite passes, including M1 pillars launched **with** the extension (pillar tests pass `Some(dir)`). **Step 5:** Commit `feat: bundled extension and native host manifest`.

### Task 1: Control socket and native host relay

**Files:** Create `src/control.rs` (socket server and ops), `src/native_host.rs`; modify `src/daemon.rs`, `src/main.rs`; `tests/m3.rs`.

**Interfaces:**
- Produces:
  - `control::serve(home, handler)`, spawned by the daemon on its `LocalSet`. It removes a stale socket file at start, creates it with mode 0600, and removes it on exit.
  - Ops in this task:
    - `list` → `[{id,title,state,waiting_on}]`, using M2b `waiting_on`;
    - `inbox`;
    - `approve`/`resume`/`pause`/`accept`/`dismiss` `{task}` → `true` or the ledger error text;
    - `ping` → `"pong"`.
  - Unknown op → `{"error":"unknown op <op>"}`.
  - `native_host::run()`: reads 4-byte little-endian length + JSON from stdin and writes it to the socket as one line. Replies go back the same way. A request over 64 KiB is answered `{"error":"request too large"}` without forwarding. A missing socket gets `{"error":"daemon not running"}`.

- [ ] **Step 1: Failing tests:**
  - `control::socket_is_owner_only`: after the daemon starts, `daemon.sock` mode is `0o600` and `lsof -nP -a -p <pid> -iTCP -iUDP` is empty.
  - `control::ops_round_trip`: over `UnixStream`, `ping` → `pong`. `list` shows a queued task. `pause {task}` → `list` shows `paused`. `{"op":"nope"}` → error `unknown op nope`. A 70 KiB line → error, and the connection stays usable.
  - `host_reports_missing_daemon`: run `iksa chrome-extension://x/` with `IKSA_HOME` lacking a socket, feed one framed `{"id":1,"op":"ping"}` → framed `{"id":1,"error":"daemon not running"}`, exit 0 on stdin EOF.
  - `host_relays_to_daemon`: daemon running, framed `ping` → framed `pong`.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** pass. **Step 5:** Commit `feat: daemon control socket and native messaging host`.

### Task 2: Side panel

**Files:** `extension/panel.html`, `extension/panel.js`, `extension/panel.css`, `extension/background.js` (`chrome.sidePanel.setPanelBehavior({openPanelOnActionClick:true})`, and the `action` key in the manifest); `tests/m3.rs`.

**Interfaces:**
- Consumes: Task 1 ops through `chrome.runtime.sendNativeMessage("dev.iksa.host", msg)`.
- Produces: the panel renders `#tasks` rows `data-id`, a `.waiting` cell and buttons `button[data-op=approve|resume|pause|accept|dismiss]`, plus `#inbox` rows. On host error it shows `#status` with the error text.

- [ ] **Step 1: Failing tests:**
  - Unit (Rust) `extension_js_has_no_network`: no file under `extension/` contains `fetch(`, `XMLHttpRequest`, `WebSocket`, `EventSource`, `importScripts` or `http://`/`https://`.
  - `#[ignore]` `m3::panel_approves_task`:
    - daemon with extension;
    - a scripted task paused for approval;
    - open `chrome-extension://<ID>/panel.html` as a normal tab via CDP;
    - within 3 s a row for the task shows `approve step`;
    - click its approve button via `Runtime.evaluate`;
    - the ledger state becomes `Queued` (then the daemon runs it).
  - `#[ignore]` `m3::panel_shows_daemon_down`: the panel with the daemon's socket removed shows `#status` containing `daemon is not running`.
- [ ] **Step 2:** FAIL. **Step 3:** implement (≈150 lines of JS; textContent only, never `innerHTML` with ledger text). **Step 4:** pass. **Step 5:** Commit `feat: side panel for tasks, inbox and approvals`.

### Task 3: Human browsing

**Files:** `src/daemon.rs`, `src/control.rs`, `src/main.rs`, `tests/m3.rs`.

**Interfaces:**
- Produces:
  - Op `browse {url?}` → the target ID. Chromium launches if needed, with the extension.
  - Daemon state `human_tabs: HashMap<String /*targetId*/, String /*url*/>`, kept from `Target.targetCreated/InfoChanged/Destroyed` with `type == "page"` and `browserContextId` == the default context.
  - `pub fn human_hosts(&self) -> Vec<String>`.
  - Idle-close requires `human_tabs.is_empty()`.
  - CLI `browse [url]` (`url` defaults to `about:blank`; a bare host gets `https://`).

- [ ] **Step 1: Failing tests (`#[ignore]`):**
  - `browse_opens_window_and_tracks_it`: `browse http://127.0.0.1:<port>/a` creates a `page` target in the default context, and `human_hosts() == ["127.0.0.1"]`. After `Page.navigate` in that tab to `/b`, the tracked URL updates.
  - `idle_close_waits_for_human_tabs`: with `idle_close = 1 s` and a human tab open, Chromium is still running after 3 s. After `Target.closeTarget`, Chromium exits within 3 s.
  - `cli::browse_without_daemon` (non-ignored): exit 1, `start iksa daemon first`.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** pass. **Step 5:** Commit `feat: human browsing in iksa's profile`.

### Task 4: Copy-on-write cookies

**Files:** `src/config.rs` (`share_cookies`), `src/engine.rs`, `tests/m3.rs`.

**Interfaces:**
- Produces:
  - `Config.share_cookies: Vec<String>` (serde default empty).
  - `pub fn sync_back(base: &[Cookie], task: &[Cookie], human: &[Cookie]) -> Vec<Cookie>`, a pure function in `engine.rs`. `Cookie` is a `serde_json::Value` subset `{name, value, domain, path, …}` keyed by `(name, domain, path)`. It returns the cookies to write: new or changed in `task` vs `base`, where `human`'s current value equals `base`'s (an absent `human` entry counts as equal only if absent from `base`).
  - The engine copies at context creation and calls `sync_back` only on `Committed`.

- [ ] **Step 1: Failing tests:**
  - Unit `sync_back_rules`:
    - task-only new cookie → written;
    - changed cookie with human unchanged → written;
    - changed in both → not written;
    - unchanged → not written;
    - deleted in task → not written.
  - Unit `human_cookie_change_wins` (the both-changed case, named for Review Focus).
  - `#[ignore]` `shared_login_reaches_agent`:
    - set cookie `session=human` for `127.0.0.1` in the default context, with `share_cookies: ["127.0.0.1"]`;
    - a task extracting `document.cookie` from the fixture sees `session=human`;
    - with `share_cookies` empty it does not.
  - `#[ignore]` `cookies_sync_only_on_commit`:
    - the fixture sets `agent=1` via JS;
    - a Committed task → the default context has `agent=1`;
    - the same flow ending `Failed` → absent.
  - `#[ignore]` `cookie_values_never_stored`: cookie value `COOKIE-CANARY-551` is absent from `ledger.db*`, every CAS blob and the report.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** full ignored suite. **Step 5:** Commit `feat: copy-on-write cookies for shared domains`.

### Task 5: Agent View and held challenge contexts

**Files:** `src/engine.rs`, `src/control.rs`, `extension/agent.html`, `extension/agent.js`, `extension/panel.js` (a "View" link per Executing/Challenge row), `tests/m3.rs`.

**Interfaces:**
- Produces:
  - The engine keeps `held: RefCell<HashMap<i64, Held { context: String, target: String, session: String, until: Instant }>>`.
    - On a Challenge pause it inserts instead of disposing.
    - `run(id)` on a task with a live hold reattaches and continues at `cursor` with no replay.
    - An expired hold is disposed by the daemon tick, and the task keeps the M1 replay path.
  - Ops:
    - `view_start {task}` → `{ok:true}`, then the daemon pushes `{"frame":"<base64 jpeg>","task":id}` lines on that connection until `view_stop` or disconnect;
    - `view_input {task, kind:"click", x, y}` | `{kind:"text", text}` | `{kind:"key", key}`.
  - Coordinates are in 1280×800 page pixels; `agent.js` scales from the `<img>` size.
  - `agent.html` needs a long-lived connection: `chrome.runtime.connectNative` port. The native host passes through multiple pushed lines per request.

- [ ] **Step 1: Failing tests (`#[ignore]`):**
  - `challenge_context_is_held`: the M1 challenge fixture pauses `Challenge`, and `Target.getTargets` still lists the task's hidden target.
  - `view_streams_frames`: over the socket, `view_start` on the held task yields ≥ 5 frames in 3 s, each decoding as JPEG (starts with `/9j/`).
  - `view_input_reaches_page`: on the fixture, the button click (V6 coordinates) changes `#out`.
  - `resume_keeps_page_state`: the challenge fixture gains a button that sets `#out` to `solved-7`. Click it through `view_input`, then `resume` and run a task whose next step is Extract `#out`. The outcome is `solved-7`, and records show no second Navigate.
  - `hold_expiry_falls_back_to_replay`: with a test hold of 1 s, wait 2 s and tick, then `resume` + run → `Committed` via replay (a repeated navigate record is present).
- [ ] **Step 2:** FAIL. **Step 3:** implement. Frames over 1 MiB are dropped and counted in the daemon log. **Step 4:** full ignored suite. **Step 5:** Commit `feat: Agent View with held challenge contexts`.

### Task 6: Page-visit triggers

**Files:** `src/schedule.rs`, `src/ledger.rs`, `src/daemon.rs`, `src/main.rs`, `tests/m3.rs`.

**Interfaces:**
- Consumes: M2b `Spec`, `schedules`, `fire_due`; Task 3 human tab updates.
- Produces:
  - `Spec::Visit(Pattern)`, parsed from `visit <host[/path]>`.
  - `Pattern::matches(&self, url: &str) -> bool`, HTTP(S) only.
  - `next_fire_ms` returns `Some(u64::MAX)` for `Visit`, so `fire_due` never fires it.
  - `Ledger::fire_visit(url: &str, now_ms: u64, never_send: &[String]) -> anyhow::Result<Vec<i64>>`: matching visit schedules with `next_ms <= now_ms` create tasks exactly like `fire_due` (mode, dedupe, `last_task`), with `{url}` substituted (query and fragment stripped), and set `next_ms = now_ms + 600_000`.
  - The daemon calls it on every human tab URL change.

- [ ] **Step 1: Failing tests:**
  - Unit `visit_pattern_matching`:
    - `visit github.com/*/pulls` matches `https://github.com/a/b/pulls` and `https://www.github.com/x/pulls?y=1`;
    - it doesn't match `https://github.com/a/issues`, `https://evilgithub.com/a/pulls` or `chrome://settings`;
    - `visit example.com` matches any path.
  - Ledger `fire_visit_cooldown_and_never_send`:
    - first match → 1 `Suggested` task whose goal contains the URL without its query;
    - an immediate second visit → none;
    - after the cooldown → one more;
    - a never_send host → none and no URL stored anywhere.
  - `#[ignore]` `human_visit_creates_suggestion`: a `browse` tab navigates to the fixture `/pulls`, and within 3 s `inbox` lists the goal.
- [ ] **Step 2:** FAIL. **Step 3:** implement. `schedule list` shows `visit …` in the spec column. **Step 4:** pass. **Step 5:** Commit `feat: page-visit triggers`.

### Task 7: Per-domain mutation lock

**Files:** `src/engine.rs`, `src/daemon.rs` (provides human hosts to the engine), `tests/m3.rs`.

**Interfaces:**
- Consumes: Task 3 `human_hosts()`, `config::domain_matches`.
- Produces: `Engine::set_human_hosts(Rc<RefCell<Vec<String>>>)`, a shared view the daemon updates. The lock check runs before Write/Financial steps only, polls at 500 ms with `LOCK_WAIT = 30 s`, and on timeout pauses with outcome `waiting: <host> is open in your tab`. A step whose `approved_step == cursor` skips the check.

- [ ] **Step 1: Failing tests (`#[ignore]`, `LOCK_WAIT` passed as a test parameter of 2 s):**
  - `lock_pauses_agent_write`: human tab on the fixture host, agent Type step → `AwaitingHandoff(Approval)` with that outcome. After `approve` and a run → `Committed`.
  - `lock_clears_when_tab_leaves`: the human tab navigates to `about:blank` 500 ms into the wait → the step proceeds, no pause.
  - `lock_ignores_reads`: a Navigate + Extract task on the same host while the tab is open → `Committed`, no wait.
  - `grant_does_not_bypass_lock`: with an M2b grant for the host, a Write step still pauses with the lock outcome.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** full ignored suite. **Step 5:** Commit `feat: per-domain lock between human tabs and agent writes`.

### Task 8: Pillars, H2 and docs

**Files:** `tests/pillars.rs`, `docs/perf.md`, `CLAUDE.md`.

- [ ] **Step 1: Re-run pillars with the extension loaded** (pillar browsers launched with `Some(extension)`):
  - idle ≤ 300 MB with one human `about:blank` window;
  - 5 tabs + 3 agents ≤ 1.2 GB (median of 3);
  - egress: zero non-loopback hosts after 60 s idle with the extension and an open side panel;
  - egress, search favicon: the manifest's `favicon_url` points at the provider's origin. Run the egress check once with Kagi active and once after opening `chrome://settings/searchEngines`; record any request to the provider host. A request at idle (no settings page) → BLOCKED with the host; a request only while the settings page is open is recorded, not a failure;
  - daemon with browser: zero TCP/UDP listeners.

  Record all numbers in `docs/perf.md` `## M3`. Over budget → BLOCKED with numbers; stop.
- [ ] **Step 2: Prepare H2 (manual; Codex stops here and prints the steps for the user).** In a temp `IKSA_HOME` with `IKSA_MOCK_KEYCHAIN` **unset**, start `iksa daemon`, then `iksa browse`, then:
  1. Note whether a "Chromium Safe Storage" Keychain prompt appears.
  2. Type `iksa test query` into the address bar; the suggestion dropdown appears.
  3. Press Enter; the result page is the configured provider (DuckDuckGo by default).
  4. Open the side panel from the toolbar icon; the task list renders.

  The user reports results; Claude records them as H2 in `docs/perf.md`.
- [ ] **Step 3: CLAUDE.md and README.md.** (User docs live only in README.md since the M2b review fixes.)
  - Module map: `extension` (generated MV3 bundle + host manifest), `control` (owner-only Unix socket ops), `native_host` (stdio↔socket relay).
  - README.md (not CLAUDE.md): `browse`, `share_cookies` (what it shares and why it defaults empty), visit triggers, the side panel, Agent View, the lock, and the expected first-run Keychain prompt.
- [ ] **Step 4:** Commit `docs: M3 pillars and README`.

---

## Done checklist

| # | Task | Check | Pass |
|---|---|---|---|
| G1 | 0 | unit + `extension_loads_and_owns_search` | Extension ID stable; provider is Default; origins restricted |
| G2 | 1 | control + host tests | 0600 socket; no TCP/UDP; host errors framed |
| G3 | 2 | panel tests + no-network grep | Approve from panel works; daemon-down message |
| G4 | 3 | browse tests | Human tab tracked; idle-close waits |
| G5 | 4 | cookie tests | Shared login reaches agent; sync only on commit; human wins; values never stored |
| G6 | 5 | Agent View tests | Held context; frames; input; resume without replay; expiry fallback |
| G7 | 6 | visit tests | Pattern, cooldown, never_send respected |
| G8 | 7 | lock tests | Writes wait/pause; reads free; grant doesn't bypass |
| G9 | 8 | pillars + H2 (user) | Budgets met with extension; H2 recorded |
| G10 | all | `ls -l target/release/iksa` | < 60 MB |

After G10: v0.1.0-alpha items remaining from the PRD are the MCP host (§3.6), `iksa update`, OpenAI-compatible and local model providers, uBlock Origin/Bitwarden checks, the research context system, and vision.
