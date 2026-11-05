# Performance and privacy evidence

Task 8, 2026-10-29. Pinned headful ungoogled-chromium 156.0.8078.47-1.1 arm64, exact Global Constraints privacy flags; `--use-mock-keychain` only in tests. Each browser test ran serially. Runtime HOME/CFFIXED_USER_HOME and TMPDIR were isolated under `.spike-home`; all measurement logs, raw netlogs, process snapshots and footprint outputs are retained in `target/codex/`.

Memory is the sum of actual `footprint -p PID -f bytes` **phys_footprint** for the browser and every transitive descendant from one `ps` snapshot, divided by 1024 (rounded down to KiB). No RSS substitutions or partial process sums. KiB/MiB are binary units; MB below is decimal. The plan's assertions use 307,200 KiB (300 MiB) idle and 1,228,800 KiB (1200 MiB) loaded.

| Check | Actual result | Evidence |
| --- | --- | --- |
| C9d, prior Task 7c | Reader/checkpoint regression passed before Task 8; commit `1d443eb` | `target/codex/v2-task7c-report.md` |
| C10 / C14b / C14c (Task 8b) | All 15 engine tests pass; 20 distinct 82,526-byte fixture pages with snapshot/CAS + audit record + cursor writes: bookkeeping median **6.336604 ms < 25 ms** | `target/codex/v2-task8b-review-green-all-ignored.log` |
| C11 | Actual `https://example.com` navigation returned 200, Example Domain title/body, and `example.com` found in cleanly flushed netlog | `target/codex/v2-task8-green-c11-before-global.log`, `positive-netlog-33013.json` |
| C12 | **0 nonloopback hosts** after fresh about:blank browser idle for 60s, following C11 positive control | `target/codex/v2-task8-green-all-ignored.log`, `idle-netlog-33401.json` |
| C13 | Three fresh launches, 10s settle each: **213,511 / 213,911 / 214,695 KiB**; median **213,911 KiB = 208.897 MiB = 219.045 MB**, below 307,200 KiB | `target/codex/v2-task8-green-all-ignored.log` |
| C14 | Three fresh loaded runs: **1,206,946 / 1,167,745 / 1,143,666 KiB**; median **1,167,745 KiB = 1140.376 MiB = 1195.771 MB**, below 1,228,800 KiB; each individual run passed | `target/codex/v2-task8-green-pillars-final.log`, `v2-task8-green-all-ignored.log`, `v2-task8-green-c14-third.log` |

C10 now measures standalone production bookkeeping in release mode: 20 distinct 82,526-byte HTML fixture pages, each timed around `snapshot::put`, `record_step` and `set_cursor`. The median of the middle two sorted samples is 6.336604 ms; all 20 snapshot trees are distinct. Engine CDP timing instrumentation and the previous engine overhead test were removed in Task 8b.

Historical Task 8 evidence (superseded C10 methodology): C10 subtracts measured elapsed time around actual CDP calls from the actual per-step wall time, beginning before the gate and including snapshot/CAS, record and cursor writes. The fixture changes a data attribute on every native innerText read and asserts 21 distinct snapshot trees plus at least 42 CAS blobs, forcing the production fsync path for the 20 measured Extract steps. It uses the real browser, DOM, CAS and SQLite. `Engine::step_overheads()` exposes those durations for this check. Earlier medians of 0.096583/0.054958 ms used dedup hits and are superseded; the intermediate unique-snapshot 0.410043 ms run exposed an approval-input race, subsequently fixed and rerun.

C11 ran standalone before the full pillar suite and again before the full ignored command. Its complete observed host set was `104.20.23.154`, `128.2.1.10`, `172.26.88.79`, `172.66.147.243`, `2606:4700:10::6814:179a`, `2606:4700:10::ac42:93f3`, `example.com`. The parser includes URL/host/address fields across all event params and excludes loopback and internal browser URL schemes. C12 checks the raw positive capture is recent, newer than the current pillar test binary, and contains `example.com`; empty or unflushed netlogs do not count as a pass.

C13 normalized startup targets to exactly one visible about:blank page. C14 used exactly five visible pages and three hidden `other` targets in three distinct agent contexts, with the original visible page reused and hidden viewports set to 1280×800. All eight URLs returned 200 with matching article titles and nonempty article DOM, followed by 20s settle: `Rust_(programming_language)`, `Linux`, `Chromium_(web_browser)`, `SQLite`, `Mozilla`, `Python_(programming_language)`, `Tokio_(software)`, `WebKit` on `en.wikipedia.org/wiki/`.

Raw evidence is retained as `target/codex/footprint-<stamp>-<pid>.log`, `footprint-<stamp>-pids.json`, `ps-tree-<root>.log`, `loaded-targets-<testpid>.json` and `idle-targets-<sample>-<testpid>.json`. Comparable loaded run roots/stamps are `32237/1793293019498736000`, `33402/1793294651408424000`, and `34190/1793294797572099000`. Idle roots/stamps in the final global run are `33657/1793294723002390000`, `33757/1793294733669789000`, `33861/1793294744371455000`.

The full required ignored command passed all **25** tests (snapshot 1, CDP 9, engine 11, pillars 4): `target/codex/v2-task8-green-all-ignored.log`. A later Type focus fix and stronger overhead fixture were covered by a final 11/11 engine run and full normal `cargo test`: `v2-task8-green-engine-focus-fixed.log`, `v2-task8-green-all-normal-final.log`; unchanged expensive pillars were not repeated after that engine-only change.

Execution history is retained in `target/codex/v2-task8-report.md`. It records the initial unsafe red-fixture launch (corrected with exact argument preflight before safe red rerun), the deliberate stale-positive-control rejection, and the approval-input failure/fix. These failed runs are not presented as passes. Task 8 removed the replaced spike kit and only the three specified old runtime paths; cleanup manifest: `target/codex/v2-task8-cleanup-manifest.log`.


Watch (M3): C14's historical worst sample was **1,206,946 KiB = 98%** of the 1,228,800 KiB budget. Recheck headroom as M3 adds browser features.


Initial Task 8b rerun before the review fix, 2026-10-29: the complete required release ignored suite passed **30/30** (audit bookkeeping 1, snapshot 1, CDP 9, engine 15, pillars 4), after a fresh standalone positive control compiled from the final test sources. C12 again observed **0 nonloopback hosts** after 60s. C13 samples were **212,471 / 212,951 / 213,111 KiB**, median **212,951 KiB < 307,200**. The current C14 loaded sample was **1,222,865 KiB < 1,228,800**, or **99.52% of budget**; all eight real Wikipedia navigations returned 200 with matching titles/DOM and five visible plus three hidden targets. This is a single Task 8b loaded sample; the prior Task 8 three-sample median remains historical evidence above. Current loaded headroom is only 5,935 KiB and merits M3 attention.

Task 8b evidence: `target/codex/v2-task8b-green-all-ignored.log`, `v2-task8b-green-normal-final.log`, `v2-task8b-green-c11.log`, and `v2-task8b-report.md`. Fresh raw captures: `positive-netlog-37272.json` (pre-suite), `positive-netlog-37671.json` (suite), `idle-netlog-37671.json`; target inventories use test PID 37671. Task 8b intentionally trims the shared footprint helper to parse/sum/assert without writing raw footprint/process logs; current measurement totals remain in the suite log, and prior raw Task 8 evidence is retained.


Final Task 8b review rerun: same-document navigation now preserves secret snapshot suppression; only a fresh Document navigation clears it. The strengthened existing secret test also covers resumed secret Type replay. Its fragment scenario failed first against the original Task 8b implementation (`v2-task8b-review-red-secret-fragment.log`), then normal tests, all 15 ignored engine tests, and the full **30/30** release ignored suite passed after the scoped privacy fix. Fresh standalone C11 passed after all final binaries were compiled. Final bookkeeping median is **6.336604 ms < 25 ms** (20 distinct 82,526-byte pages); snapshot median **9.025542 ms**. Final C12 observed **0 nonloopback hosts** after 60s. C13 samples **206,823 / 218,663 / 221,207 KiB**, median **218,663 KiB < 307,200**. Final C14 loaded sample **1,207,361 KiB < 1,228,800** (**98.26% of budget**, 21,439 KiB headroom). The earlier Task 8b run above is separate pre-review evidence, not the final validation.

Final logs: `target/codex/v2-task8b-review-green-{normal,engine,build,c11,all-ignored}.log`; final raw captures `positive-netlog-39086.json` (pre-suite), `positive-netlog-39545.json` (suite), `idle-netlog-39545.json`, with target inventories using PID 39545. `v2-task8b-review-c14d.log` again records 0/0. Commit remains pending parent review/commit.

Task 9a final validation, 2026-10-29: shared Chrome fixtures and non-HTTP navigation passed normal `cargo test` (**62 passed, 31 ignored**) and the complete release ignored suite (**31/31**: 2 library, 9 CDP, 16 engine, 4 pillars). The new about:blank/data regression first failed its 10s deadline, then passed; the existing secret test also verifies suppression across fragments and restoration of snapshots on a fresh data document. C14f has no matches.

Current bookkeeping median is **6.169667 ms < 25 ms**, snapshot fresh-put median **8.903084 ms**. C11 returned Example Domain 200 and found `example.com` in its flushed netlog. C12 now runs its own fresh live positive control immediately before its separate 60s idle capture; it observed **0 nonloopback hosts**, with no stale marker dependency. C13 three fresh launches yielded sorted **213,191 / 213,495 / 216,551 KiB**, median **213,495 KiB < 307,200**. C14 three independent loaded launches (full suite plus two focused runs), each with all eight real Wikipedia responses 200, matching article titles/DOM and five visible plus three hidden targets, yielded **1,152,706 / 1,226,530 / 1,129,345 KiB**; median **1,152,706 KiB < 1,228,800**. All individual assertions passed; the largest run had 2,270 KiB headroom. Limits are unchanged.

Evidence: `target/codex/9a-{red,normal-final,engine,all-ignored,loaded2,loaded3}.log` and `target/codex/v2-task9a-report.md`. Tests now write netlogs/inventories only inside per-test temporary directories under isolated `.spike-home/tmp`, removed on drop; host sets, target inventories and measurement totals remain printed in the Codex logs. Tests never write to Codex scratch. Task 9a commit pending parent review.

Task 9 final validation, 2026-10-29: `cargo test` passes (60 lib, 11 cli + 1 ignored, others) and the complete release ignored suite passes (2 lib, 9 CDP, 1 cli `e2e_report`, 16 engine, 4 pillars). Release binary `ls -l target/release/iksa` = **4,316,240 bytes** (< 60 MB). CAS after `e2e_report`, `du -sk` = **268 KiB** (< 500). C11 found `example.com`; C12 observed **0 nonloopback hosts** after 60s; C13 samples **209,799 / 212,599 / 212,967 KiB**, median **212,599 KiB < 307,200**; C14 loaded sample **1,117,778 KiB < 1,228,800**.

Pillar fix: `idle_egress_is_loopback_only` once saw 2 page targets because `PillarBrowser::normalize` closed pages once and created one, racing Chromium's startup tab. It now polls until exactly one `about:blank` page is stable for 5 consecutive checks (closing extras, creating if absent); the 60s zero-host and one-page assertions are unchanged. Log: `target/codex/claude-full-ignored.log`.

## Manual

**H1 — real setup + Keychain (2026-10-29, user's normal Terminal, `IKSA_MOCK_KEYCHAIN`/`CHROMIUM`/`IKSA_HOME` unset):** PASS.
- `iksa setup` downloaded, SHA-256-verified and installed the pinned build to `~/.iksa/chromium/`; the path printed.
- `iksa run` against `https://example.com` committed (task 2). Task 1 failed `timeout at step 1` because example.com no longer has an `<h1>` (its body is a single `<p>`); a crash-resume re-check with `Extract p` committed in 0.66 s total.
- A Ctrl-C mid-run left a task `Executing`; the next `run` recovered it as designed.
- **No Keychain prompt** on any run. Expected: agent tasks run in off-the-record browser contexts, so nothing is encrypted to disk. The first real prompt should come with human browsing in iksa's profile (M3).
- UX found: `add -` waits silently on a terminal; `run` with an empty queue prints nothing and still launches Chromium; timeout messages don't name the selector. Fixed in M2's first commit.

**H2 — address bar with memory flags:** deferred to M3. Not run manually; it only affects human browsing in iksa's own profile, which M3 introduces. M3's plan carries it as an automated or guided check with the bundled extension.

**M1 closed 2026-11-03.**


## M2a planner eval — preparation (2026-11-04)

**D9: NOT RUN.** The live evaluation is awaiting explicit API-spend approval. No live score, token usage, or actual cost is claimed below. The harness and fixtures are in `tests/eval.rs` and `tests/eval/`.

The manifest contains 15 local fixture goals and five public, read-only goals. The initial navigation is an ordinary ledger step. Real-site write actions require approval; an approval pause fails those read-only goals. Reference values for the public pages are captured before the run in the same isolated Chromium process; Hacker News and DuckDuckGo are captured last and evaluated first to reduce drift. Missing references count as failed tasks, with no fabricated expected answer.

| # | Goal | Live success | Steps | Input / output tokens | USD |
|---|---|---|---|---|---|
| 1 | Search form | Not run | — | — | — |
| 2 | Multi-page navigation | Not run | — | — | — |
| 3 | Pagination | Not run | — | — | — |
| 4 | Table cell | Not run | — | — | — |
| 5 | Dismiss modal | Not run | — | — | — |
| 6 | Fix validation | Not run | — | — | — |
| 7 | Named secret login | Not run | — | — | — |
| 8 | Financial approval | Not run | — | — | — |
| 9 | CAPTCHA handoff | Not run | — | — | — |
| 10 | Distractor links | Not run | — | — | — |
| 11 | Native select | Not run | — | — | — |
| 12 | Checkbox and terms | Not run | — | — | — |
| 13 | Same-page anchor | Not run | — | — | — |
| 14 | Impossible goal | Not run | — | — | — |
| 15 | Blocked domain | Not run | — | — | — |
| 16 | Wikipedia fact | Not run | — | — | — |
| 17 | Hacker News top title | Not run | — | — | — |
| 18 | Example.com paragraph | Not run | — | — | — |
| 19 | DuckDuckGo result | Not run | — | — | — |
| 20 | MDN property fact | Not run | — | — | — |

The grade requires the expected terminal state and answer string, or the expected approval/challenge pause. Validation-error and same-page-anchor cases also require evidence in a subsequent model request. Financial approval must pause on a pending Financial click. Native select behavior is measured as implemented; the fixture and grade do not bypass missing product capabilities. The live test asserts that all 20 cases were attempted, plus secret and never_send privacy invariants; it does not assert a success threshold. D9 requires at least 16/20 on Opus 5.5.

Usage is captured by a test-only loopback relay forwarding the exact Messages request to the Anthropic endpoint. It preserves retries and drains in-flight work before reading measurements. API keys are forwarded in headers and are not logged. Prompt token totals include uncached input, cache creation and cache reads. USD is a valuation of returned usage at the verified public rates, not an invoice. Missing usage remains unknown. Fallback/refusal billing, including possible credits, is marked unknown rather than assigning an invented cost; per-attempt usage is retained in the JSON output.

The offline harness runs local fixtures with mock model replies, skips the five external sites, verifies login/validation/checkbox interactions with scripted browser actions, and labels every row **not scored**. Offline tokens are explicitly mock data. They are not live evaluation results.

### Estimated API spend for approval

Rates checked against [Anthropic's pricing documentation](https://platform.claude.com/docs/en/about-claude/pricing) on 2026-11-04: Opus 5.5 input $4/MTok, output $20/MTok, 5-minute cache writes $5/MTok and cache reads $0.20/MTok. [Prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching) applies only when the prefix qualifies and cache entries remain valid. [Fallback billing](https://platform.claude.com/docs/en/build-with-claude/refusals-and-fallback) can add attempts or credits.

Assume 20 goals × 8 turns = 160 calls, about 6,000 input tokens/call (960,000 total), and a reusable 1,000-token prefix. With one cache write and 159 hits, 400 output tokens/call gives **about $4.52**; with no cache, **$5.12**. At 1,000 output tokens/call these become **$6.44 / $7.04**. At the full 4,096-token output limit, 160 calls cost about **$16.95** without caching. If all 20 goals reach 25 turns, the same input estimate and full output limit give about **$52.96**, before retries or fallback effects. These are estimates, not spend caps.

### Running after approval

`live_eval` is ignored and additionally requires `IKSA_LIVE_EVAL_APPROVED=1` and an existing `ANTHROPIC_API_KEY` environment variable. It never reads the real Keychain: the demo secret uses the isolated fake Keychain. It uses `claude-opus-5-5`, effort `low`, 25 turns maximum per goal. This command is recorded for later use; it has **not** been run:

```sh
IKSA_LIVE_EVAL_APPROVED=1 CHROMIUM=$PWD/.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium cargo test --release --test eval live_eval -- --ignored --test-threads=1 --nocapture
```

The run prints a Markdown table and one `EVAL_RESULT` JSON object. After a user-approved run, replace the pending table with its actual measurements and record success totals, privacy assertions, cost, and any unpriced usage. Only then make Task 7's live-results commit. A result below 16/20 is reported as below target and belongs to M2a-tune; grades must not be weakened.

Offline validation on 2026-11-04: all **115 regular tests** and **52 non-live ignored tests** passed, including nine eval helper checks and the offline Chromium smoke run. Both commands excluded `live_*`. These results validate the harness and existing product; they do not establish a live score or D9 pass.

## M2b

Measured on 2026-11-05 with the release build and the pinned Chromium. The daemon
checks start a fresh `iksa daemon` with an empty ledger in an isolated temporary
data directory, wait ten seconds, verify it is still running, and stop it with
SIGTERM. Tests write their artifacts only in their temporary directories; the
local command logs below are Codex scratch, not test output paths.

| Check | Actual result | Local evidence |
| --- | --- | --- |
| E9 idle daemon footprint | Three fresh processes: **2,480 / 2,400 / 2,464 KiB**; median **2,464 KiB** (2.40625 MiB), below **30,720 KiB**. Every process had **zero descendants**, including no Chromium child. | `target/codex/m2b-task8-final-daemon-pillars.log` |
| E9 idle daemon egress | After ten seconds, `lsof -nP -a -p <pid> -i` returned **status 1, zero stdout bytes and zero stderr bytes**: no Internet sockets. The daemon remained alive with no descendants. | `target/codex/m2b-task8-final-daemon-pillars.log` |
| E5 bounded concurrency | Four goals: `max_agents=3` reached **peak 3** and finished in **3.092757042 s**, below the unchanged **3.5 s** bound. `max_agents=1` reached **peak 1** and finished in **4.812079042 s**. All goals committed without duplicate dispatch. | `target/codex/m2b-task8-concurrency.log` |
| E10 release binary | **7,309,408 bytes**, below **60,000,000 bytes**. | `ls -l target/release/iksa` |

The footprint uses `common::tree_footprint_kib`: summed process-tree
`phys_footprint`, not RSS. The daemon's readiness probe waits for creation of its
fresh ledger before checking that its run lock is held, so the probe cannot
compete with the initial lock acquisition. Socket checking distinguishes the
expected no-match exit status from an `lsof` error. Limits and pre-existing M1
pillar assertions are unchanged.

Reproduce the focused measurements serially:

```sh
CHROMIUM=$PWD/.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium cargo test --release --test pillars idle_daemon_ -- --ignored --test-threads=1 --skip live_ --nocapture
CHROMIUM=$PWD/.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium cargo test --release --test daemon max_agents_bounds_concurrency -- --ignored --test-threads=1 --skip live_ --nocapture
ls -l target/release/iksa
```

Independent acceptance checks are kept unchanged. Validation uses
`cargo test --test acceptance_m2b -- --skip live_`,
`cargo test -- --skip live_`, and the complete ignored suite:

```sh
CHROMIUM=$PWD/.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium cargo test --release -- --ignored --test-threads=1 --skip live_
```

These measurements use local model mocks. No `live_*` test or `live_eval` was
run; the M2a D9 live score remains unmeasured.

Final validation on 2026-11-05 passed **185 regular tests**, including all
**22 unchanged independent acceptance checks**, and **76 non-live ignored
tests**, including both new daemon pillars and all four existing M1 pillars.
The focused two-pillar measurement and concurrency benchmark also passed.
Evidence: `target/codex/m2b-task8-{acceptance,full,ignored}.log`,
`m2b-task8-final-daemon-pillars.log`, and `m2b-task8-concurrency.log`.
The LaunchAgent example passed `plutil -lint`; it was not installed or loaded.
