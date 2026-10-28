# Spike 2 — Unblock M1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clear the two Spike 1 blockers (memory FAIL, hidden-target PARTIAL) with correct measurements and a CDP client that can drive hidden targets, then rewrite M1 around the results.

**Architecture:** Three re-measurements reuse the existing throwaway `spikes/` harness. One real module, `src/cdp.rs`, replaces chromiumoxide: a minimal CDP client over `--remote-debugging-pipe` (Chromium's fd 3/4 transport, NUL-delimited JSON). No TCP port, flattened sessions, so hidden `type: "other"` targets work like any other.

**Tech Stack:** Rust 1.97 (edition 2024), tokio (`rt-multi-thread`, `macros`, `sync`, `time`), serde_json, anyhow, libc (for `dup2` in `pre_exec`). Python 3 stdlib for spike scripts.

**Spec:** `gemini-code-1793212240267.md` (PRD; renamed `PRD.md` later), `docs/superpowers/plans/2026-10-28-agentic-browser-m1-agent-core.md` (M1 plan), `docs/spikes.md` + `docs/briefs/BRIEF_1_RESULT.md` (Spike 1 evidence).

## What happened in Spike 1

| Spike | Verdict | Why it matters | Root cause / gap |
| --- | --- | --- | --- |
| S1 ungoogled-chromium 156.0.8078.47-1.1 arm64 | PASS | Engine pin works | — |
| S2 hidden targets | PARTIAL | Agents can't run unseen via chromiumoxide; Agent View got 0 frames | chromiumoxide 0.9.1 `get_page` ignores `type: "other"` targets. Raw CDP works. Frames probe set no viewport and acked no frames |
| S3 egress | PASS (weak) | Privacy pillar | One `lsof` snapshot after 60 s; misses short-lived connections |
| S4 extension + native host | PARTIAL | Search override + UI channel work | Only the first-run confirmation bubble went unobserved (no window capture) |
| S5 memory | FAIL | Efficiency pillar | Metric was summed RSS, which counts shared pages once per process (spec error, mine). Idle also preloads `chrome://omnibox-popup.top-chrome` |
| S6 bot visibility | PASS | Hand-off load | Turnstile *widget* on a page that loaded fine (200). M1 rule 2 would pause on every page that embeds Turnstile |
| Keychain popup | worked around | Privacy at rest | Seen when Codex launched Chromium; `--use-mock-keychain` used for spikes only |

## Decisions this plan makes

- **Drop chromiumoxide; own a pipe CDP client** (Task 4). It fixes S2, removes the loopback TCP port (PRD §4.4), and avoids chromiumoxide's arg mangling (`----disable-sync`) and forced `--disable-extensions`. Cost: about 150 lines we own. Clicks/typing go through `Runtime.evaluate` + `Input.insertText`/`Input.dispatchMouseEvent`, which Spike 1 proved on a hidden target.
- **Memory metric = summed `phys_footprint`** (macOS `footprint`, matching Activity Monitor's Memory column), median of 3 runs. Targets stay 300 MB idle / 1.2 GB loaded until Task 1's numbers are in; Task 6 brings real numbers to the user if they're missed. No silent relaxing.
- **Turnstile rule refined** (applied in Task 6 to M1 Task 5): Turnstile counts as a challenge only if, after 10 s, no `input[name="cf-turnstile-response"]` has a non-empty value. Invisible/managed passes don't pause the agent.

## Global Constraints

- Chromium pin: ungoogled-chromium `156.0.8078.47-1.1` arm64, SHA-256 `5439d292078e260928d202145d0a20f19f227424888e84beb46973c16d40a186`, installed at `.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium`.
- Spike runs: everything under `.spike-home/`, browsers run serially, `--use-mock-keychain` allowed only in spikes and tests.
- Privacy launch args = Spike 1's final list (`docs/spikes.md` "Pins and final launch contract"), minus `--remote-debugging-address/port`, plus `--remote-debugging-pipe` for the new client.
- No new dependencies beyond the Tech Stack line.
- Never write outside `iksa/` (except cargo's registry cache).

## Review Focus

- **Chromium exits or crashes mid-call**: pending `call`s must fail fast, not hang. Test `browser_exit_fails_pending` (Task 4).
- **Messages larger than one pipe read** (e.g. a 1.4 MB `outerHTML`): framing must reassemble them. Tests `split_frames_*` and `large_eval_roundtrip` (Task 4).
- **Event fired before anyone listens** (`Page.loadEventFired` racing `Page.navigate`): callers subscribe first. Test `hidden_target_navigate_eval_type` subscribes before navigating (Task 4).
- **iksa dropped or panics with Chromium running**: no orphan browser. Test `drop_kills_browser` (Task 4).
- **Noisy memory numbers**: one sample can swing ±20%. Task 1 takes the median of 3 fresh-profile runs.

---

### Task 1: Memory, re-measured correctly, plus reduction levers (spike)

**Files:** Modify `spikes/measure.py`, `spikes/raw_probe.py` (s5 mode); append results to `docs/spikes.md` under `## Spike 2 — S5'`.

- [x] **Step 1:** `measure.py` additionally prints `FOOTPRINT_KiB=<sum>` over the same PID set, from `footprint -p <pid>` for each PID (parse the `phys_footprint:` line; skip helper PIDs `python`/`ps`/`footprint`). Keep `RSS_KiB` so the two can be compared.
- [x] **Step 2:** `raw_probe.py s5` takes extra Chromium args from env `SPIKE_ARGS` (space-separated, bare keys) and repeats the idle + hidden-Wikipedia measurement 3 times, each on a fresh profile. Print the median of each.
- [x] **Step 3:** Run these configs (each = Spike 1 args + listed extras):
  - A baseline: none
  - B: `--renderer-process-limit=2`
  - C: `--process-per-site`
  - D: whatever `--disable-features=` value stops the idle `chrome://omnibox-popup.top-chrome` WebUI preload (find the feature name in Chromium 156's `chrome://flags` or the source of `top_chrome` WebUI preloading; record the name used)
  - E: B + C + D
- [x] **Step 4:** Record a table in `docs/spikes.md`: config, idle median footprint MB, loaded median footprint MB, idle RSS MB. Mark PASS if any config is ≤ 300 MB idle and ≤ ~600 MB with one hidden agent.
- [x] **Step 5:** Commit `docs: S5' memory by phys_footprint and levers`.

**Verifiable:** `docs/spikes.md` table has 5 rows × 3 numbers; each number traces to a line `FOOTPRINT_KiB=` in `.spike-home/logs/s5-<config>-<run>.log`.

### Task 2: Egress via netlog (spike)

**Files:** Modify `spikes/raw_probe.py` (new mode `s3`); results under `## Spike 2 — S3'`.

- [x] **Step 1:** `raw_probe.py s3` launches with `--log-net-log=.spike-home/logs/netlog-s3.json --net-log-capture-mode=Default`, idles 120 s on `about:blank`, closes the browser cleanly (`Browser.close`) so the netlog is flushed.
- [x] **Step 2:** Same script parses the netlog JSON and prints every distinct host from `URL_REQUEST_START_JOB` / `SOCKET` / `HOST_RESOLVER` events, then `NON_LOOPBACK=<n>`.
- [x] **Step 3:** Run with and without the loaded spike extension (`--load-extension=spikes/ext`).
- [x] **Step 4:** Record both host lists; PASS = `NON_LOOPBACK=0` in both. For any host found, record the flag or pref that removes it and re-run.
- [x] **Step 5:** Commit `docs: S3' egress via netlog`.

**Verifiable:** `grep NON_LOOPBACK .spike-home/logs/s3-*.log` prints `NON_LOOPBACK=0` twice.

### Task 3: Hidden-target screencast and input (spike)

**Files:** Modify `spikes/raw_probe.py` (`s2` mode); results under `## Spike 2 — S2'`.

- [x] **Step 1:** In `CDP.frame()`, when a message's `method == "Page.screencastFrame"`, send `Page.screencastFrameAck { sessionId: params.sessionId }` on the same session before returning.
- [x] **Step 2:** On the hidden target, before `Page.startScreencast`, call `Emulation.setDeviceMetricsOverride { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false }`. Count frames over 6 s with the animated page (same as Spike 1).
- [x] **Step 3:** Also try `Emulation.setFocusEmulationEnabled { enabled: true }` if Step 2 yields 0 frames, and record each variant separately.
- [x] **Step 4:** Input forwarding check on the hidden target: `Input.dispatchMouseEvent` (`mousePressed` + `mouseReleased`, `button: "left"`, `clickCount: 1`) at the `#go` button's center (from `DOM.getBoxModel` or `getBoundingClientRect`) after `Input.insertText("forwarded")`; read `#out`.
- [x] **Step 5:** Record: frames per variant (hidden), frames (visible control, now acked), `#out` value. PASS = hidden frames > 0 and `#out == "forwarded"`.
- [x] **Step 6:** Commit `docs: S2' hidden screencast with viewport + ack`.

**Verifiable:** log lines `hidden_frames_<variant>=<n>`, `visible_control_frames=<n>` (expect ≥ 30 in 6 s now that frames are acked), `forwarded_out=forwarded`.

### Task 4: Crate scaffold + pipe CDP client (production code)

**Files:**
- Create: `Cargo.toml` (package `iksa`), `src/main.rs` (`mod cdp; fn main() {}`), `src/cdp.rs`, `tests/cdp.rs`
- Modify: `.gitignore` (add `/target`)

**Interfaces — produces `cdp`:**
- `pub struct Event { pub method: String, pub params: serde_json::Value, pub session_id: Option<String> }` (Clone)
- `pub struct Browser` — owns the `std::process::Child`; `Drop` kills and reaps it.
- `pub fn launch(exe: &Path, args: &[String]) -> anyhow::Result<Browser>` — appends `--remote-debugging-pipe`; child fd 3 = our write pipe's read end, fd 4 = our read pipe's write end (`std::io::pipe()` + `libc::dup2` in `CommandExt::pre_exec`); reader on a `std::thread`.
- `impl Browser`:
  - `pub async fn call(&self, method: &str, params: Value, session: Option<&str>) -> anyhow::Result<Value>` — returns `result`; CDP `error` -> `Err` whose message contains the CDP `code` and `message`; browser gone -> `Err("browser closed")`.
  - `pub fn events(&self) -> tokio::sync::broadcast::Receiver<Event>` (capacity 1024).
  - `pub fn pid(&self) -> u32`.
- `pub(crate) fn split_frames(buf: &mut Vec<u8>) -> Vec<Vec<u8>>` — drains every complete NUL-terminated frame from `buf`, leaves the partial tail.

Message routing: frames with `id` complete the matching pending `tokio::sync::oneshot` (map behind `std::sync::Mutex<HashMap<u64, Sender>>`); frames with `method` go to the broadcast; on EOF every pending sender gets `Err("browser closed")`.

- [ ] **Step 1: Failing unit tests in `src/cdp.rs`:**
  - `split_frames_partial`: `b"{\"a\":1}\0{\"b\""` -> one frame `{"a":1}`, buffer keeps `{"b"`.
  - `split_frames_many`: three frames in one buffer -> three, buffer empty.
  - `split_frames_none`: no NUL -> empty vec, buffer unchanged.
- [ ] **Step 2: Failing integration tests in `tests/cdp.rs`**, all `#[ignore]`, browser from env `CHROMIUM`, Spike 1 args + `--use-mock-keychain` + temp 0700 profile under `std::env::temp_dir()`; fixture = 20-line `TcpListener` thread serving the Spike 1 form (`#q`, `#go`, `#out`):
  - `version_over_pipe`: `Browser.getVersion` -> `product` starts with `Chrome/156`.
  - `no_tcp_listener`: `lsof -nP -a -p <pid> -iTCP -sTCP:LISTEN` output is empty.
  - `hidden_target_navigate_eval_type`: create context; `Target.createTarget { url: "about:blank", hidden: true, browserContextId }`; `Target.attachToTarget { flatten: true }` -> session; `Page.enable`; subscribe to `events()`; `Page.navigate` to fixture; wait for `Page.loadEventFired` on that session (5 s timeout); `Runtime.evaluate` focus `#q`; `Input.insertText("pipe typed")`; JS click `#go`; `#out` textContent == `"pipe typed"`.
  - `large_eval_roundtrip`: `Runtime.evaluate { expression: "'x'.repeat(2_000_000)", returnByValue: true }` -> value length 2,000,000.
  - `cdp_error_is_err`: `call("Nope.nope", json!({}), None)` -> `Err` whose string contains `-32601`.
  - `browser_exit_fails_pending`: kill pid with SIGKILL, then `Browser.getVersion` returns `Err` containing `browser closed` within 2 s.
  - `drop_kills_browser`: drop `Browser`; within 2 s `kill(pid, 0)` fails (process gone).
- [ ] **Step 3:** `cargo test cdp` (unit) and `CHROMIUM=$PWD/.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium cargo test --test cdp -- --ignored --test-threads=1` — FAIL.
- [ ] **Step 4:** Implement.
- [ ] **Step 5:** Same two commands — 3 unit + 7 integration pass.
- [ ] **Step 6:** `cargo build --release && ls -l target/release/iksa` — record size in commit message.
- [ ] **Step 7:** Commit `feat: pipe-transport CDP client (replaces chromiumoxide)`.

### Task 5: Manual checks (user, about 5 minutes)

Run from a normal Terminal, not inside Codex. Results go in `docs/spikes.md` under `## Spike 2 — manual`.

- [ ] **M1 Keychain:** launch `.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium --user-data-dir=$PWD/.spike-home/profile-manual --no-first-run` (no mock keychain). Expected: at most one "Chromium Safe Storage" prompt; after **Always Allow**, a relaunch shows none. Record what appeared.
- [ ] **M2 Search bubble:** same command plus `--user-data-dir=$PWD/.spike-home/profile-bubble --load-extension=$PWD/spikes/ext`. Record whether a "Change search settings?" bubble appears, and whether typing a query in the address bar lands on duckduckgo.com.

### Task 6: Decision gate + doc updates (planner, after Tasks 1–5)

- [ ] **Step 1: Gate.** Go to M1 only if: Task 4 is all green, Task 2 shows `NON_LOOPBACK=0`, and Task 1 has a config at or under target, or the user has accepted new targets based on Task 1's numbers. If Task 3 fails, bring the user the hand-off fallback choice (visible target in the same context from the start of a hand-off, losing page state) before M3 planning.
- [ ] **Step 2: PRD.** §5 memory row defined as summed phys_footprint, median of 3; §4.4 marked resolved (pipe transport); §1.2.1 notes the winning memory flags.
- [ ] **Step 3: M1 plan.**
  - Tech Stack: chromiumoxide out; `libc` in.
  - Task 2: CAS only (scaffold done here).
  - Task 5: refined Turnstile rule + test `turnstile_auto_passed_is_not_challenge` (`cf-turnstile` div plus `<input name="cf-turnstile-response" value="tok">` -> `None`). The engine re-checks after 10 s.
  - Task 8: built on `cdp::Browser`; actions map to Page/Runtime/Input calls; launch args from Task 1's winning config.
  - Task 9: drop the `update` command (`setup` reinstalls on pin mismatch); arm64-only pin; Task 4 drops `max_agents` (moves to M2). All from the audit.
- [ ] **Step 4: Cleanup.** `git rm -r spikes/src spikes/Cargo.toml spikes/Cargo.lock spikes/capture_window.py` and delete the `BRIEF_1_RESULT.md` duplicate table. Keep `raw_probe.py`, `measure.py`, `prepare.py`, `native-host.py`, `ext/` as the reproduction kit.
- [ ] **Step 5:** Commit `docs: spike 2 decisions; M1 plan updated`.

## Checklist of verifiable cases

| # | Case | Command / evidence | Pass |
| --- | --- | --- | --- |
| V1 | Idle memory, correct metric | `docs/spikes.md` S5' table | some config ≤ 300 MB idle median |
| V2 | Agent memory | same table | ≤ ~600 MB with one hidden agent |
| V3 | Egress, no extension | `s3` log | `NON_LOOPBACK=0` |
| V4 | Egress, with extension | `s3` log | `NON_LOOPBACK=0` |
| V5 | Hidden screencast | `s2` log | `hidden_frames_*` > 0 |
| V6 | Hidden input forwarding | `s2` log | `forwarded_out=forwarded` |
| V7 | Framing | `cargo test cdp` | 3 unit pass |
| V8 | Pipe CDP end to end | `cargo test --test cdp -- --ignored --test-threads=1` | 7 pass |
| V9 | No TCP port | `no_tcp_listener` | pass |
| V10 | Crash safety | `browser_exit_fails_pending`, `drop_kills_browser` | pass |
| V11 | Keychain prompt | manual M1 | ≤ 1 prompt, none after Always Allow |
| V12 | Search bubble | manual M2 | recorded; DDG search works |
