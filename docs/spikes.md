# Task 1 — feasibility spike results

Run date: 2026-10-28. Machine: Darwin 27.0.0, arm64, Rust/Cargo 1.97.1.
Scope: throwaway `spikes/` only; no Task 2+, production crate, or PRD rename.

| Spike | Result | Evidence / decision |
| --- | --- | --- |
| S1 Chromium | PASS | SHA-256 verified; headful CDP returns Chrome/156.0.8078.47. |
| S2 Hidden targets | PARTIAL | Raw CDP works; Rust Page subcheck BLOCKED: `Requested value not found.` / `deadline has elapsed`; hidden frames 0, visible 3. |
| S3 Idle egress | PASS | After 60 s, only 127.0.0.1 CDP sockets; no non-loopback endpoint observed. |
| S4 Extension / native host | PARTIAL | Effective DDG default and native ping-pong verified; fresh-profile bubble observation BLOCKED: `No on-screen scoped Chromium windows available`. |
| S5 Memory | FAIL | Idle 1,015,952 KiB; hidden Wikipedia 1,322,544 KiB; idle exceeds ~300 MB. |
| S6 Bot visibility | PASS | Both visible and hidden show Turnstile markers; no interstitial; observation only. |

## Reproduction and evidence

Build from repository root:

```sh
TMPDIR="$PWD/.spike-home/tmp" CARGO_TARGET_DIR="$PWD/spikes/target" cargo build --manifest-path spikes/Cargo.toml
python3 spikes/prepare.py
python3 -m http.server 8765 --bind 127.0.0.1 --directory .spike-home/www
```

The common Rust builder enforces HOME/CFFIXED_USER_HOME/TMPDIR on the child via `BrowserConfigBuilder.env`, without mutating the multithreaded runtime process environment. Raw Python launch also explicitly supplies them. Optional `SPIKE_PROFILE_NAME` accepts only a single safe directory name and creates a 0700 profile inside `.spike-home`. All final browser launches set `HOME=$PWD/.spike-home`, `CFFIXED_USER_HOME=$PWD/.spike-home`, and `TMPDIR=$PWD/.spike-home/tmp`; all use `--use-mock-keychain`. Run each browser serially. Rust dependency lock resolves chromiumoxide 0.9.1. Python probes use only stdlib, to separate CDP capability from crate limitations; they are throwaway, not a proposed second production driver. Runtime logs and HTML are in ignored `.spike-home/logs/`; excerpts below are the reviewable evidence retained in this document.

Deviation: the first S1 browser launch preceded HOME redirection and the mock-keychain flag. It still used the explicit `.spike-home/profile`; the user reported a missing-keychain modal. That launch is excluded from final results; all browser experiments were rerun with isolation and mock keychain. A later read-only check found the existing real-home `Library/Application Support/Chromium/NativeMessagingHosts` directory dated Aug 23; real-home `Library/Caches/Chromium` and `Library/Preferences/org.chromium.Chromium.plist` did not exist. No outside-path write was established by that check, and none was removed. The OS may maintain its own application metadata; strict absence of such side effects was not audited. No user browser/profile was used.

## S1 — PASS: pinned ungoogled-chromium launches

Latest release metadata was fetched from the repository releases API and saved to `.spike-home/downloads/latest.json`. Both the published release-body SHA-256 and asset `digest` match the completed 143 MiB download.

```sh
curl -fL --retry 2 -o .spike-home/downloads/chromium.dmg <pinned URL below>
shasum -a 256 .spike-home/downloads/chromium.dmg
hdiutil attach -nobrowse -mountpoint "$PWD/.spike-home/tmp/mount" .spike-home/downloads/chromium.dmg
cp -R .spike-home/tmp/mount/Chromium.app .spike-home/chromium/
hdiutil detach "$PWD/.spike-home/tmp/mount"
xattr -dr com.apple.quarantine .spike-home/chromium/Chromium.app
spikes/target/debug/s1
```

Final `.spike-home/logs/s1-final.log`:

```text
protocol_version: "1.3"
product: "Chrome/156.0.8078.47"
revision: "@1d17a2e2ee147bb7d6ae2ab6abdb3a2510c17775"
js_version: "15.6.75.19"
```

The Rust builder uses `.disable_default_args().with_head()`, explicit executable and profile. Profile mode observed 0700. Review verification `spikes/target/debug/s1 --verify-env` was invoked without caller HOME exports; its `/usr/bin/env` child (using the same `config.process_envs` map passed by Chromium launch) printed only the three scoped HOME/CFFIXED_USER_HOME/TMPDIR values, then CDP again returned the pinned version (`child-env-verified.log`). The user observed a macOS “keychain cannot be found” popup on every pre-fix launch; `--use-mock-keychain` was added to **every** launch site and all six final experiments rerun. This is a disposable-profile workaround using a fixed encryption key, not a production credential-storage choice.

## S2 — PARTIAL: protocol works; chromiumoxide Page integration fails

Commands: `spikes/target/debug/experiments s2` and `python3 spikes/raw_probe.py s2` (local fixture above). Both explicitly create a non-default browser context and `hidden: true` target.

High-level `browser.new_page(hidden_params)` timed out at a 15 s caller deadline (`deadline has elapsed`). To avoid treating this as a protocol failure, the final Rust probe used `browser.execute(CreateTargetParams)` and queried targets:

```text
raw_hidden_target=TargetId(...)
raw_window=Err(Chrome(Error { code: -32000, message: "Browser window not found" }))
TargetInfo { type: "other", url: "about:blank", attached: false, browser_context_id: Some(...) }
RESULT=Err(Requested value not found.)
```

`browser.get_page(id)` cannot return this hidden target in chromiumoxide 0.9.1. **Rust Page integration subcheck: BLOCKED**, exact errors `deadline has elapsed` (15 s caller deadline) and `Requested value not found.` (`get_page`). Consequently `Page.content()` and element `.click().type_str()` through that crate are **not proven**; this prevents a full PASS.

The independent stdlib CDP probe explicitly attaches to the hidden target, enables Page, navigates to the localhost form, and retrieves the full DOM with `Runtime.evaluate(document.documentElement.outerHTML)`. `Input.insertText("raw typed")` plus a DOM button `.click()` yields output `raw typed`. This proves protocol navigation/DOM/typing/DOM click, not the unavailable chromiumoxide element API or a physical mouse dispatch.

With an animated background toggling every 100 ms and a 6 s sample:

```text
hidden_window_error {"code": -32000, "message": "Browser window not found"}
screencast_frames 0
activate {}
activated_window_error {"code": -32000, "message": "Browser window not found"}
handoff_window {'windowId': ..., 'bounds': {..., 'windowState': 'normal'}}
handoff_cookie {'result': {'type': 'string', 'value': 'spike=same-context'}}
visible_control_frames 3
```

The visible positive control used the same animation and `Page.startScreencast`; received frames were acknowledged. Hidden absence is not a static-page ambiguity. The cheap tested handoff is a **new visible target in the same browser context**, keeping cookies. It does not make the original target visible or transfer DOM/heap/form/challenge state. No OS screenshot/Accessibility window inspection was used; CDP explicitly reports no browser window.

## S3 — PASS: 60 s idle socket snapshot is loopback only

Command: `spikes/target/debug/experiments s3`. Explicit privacy args below, headful, about:blank, 60 s idle, then `ps -axo pid=,ppid=,rss=,command=` and `lsof -nP -a -p <driver and descendant PIDs> -i`. The retained process listing records actual emitted flags and process ancestry; it contains no `--enable-automation`.

Final `.spike-home/logs/s3-final.log`:

```text
experimen 99432 ... TCP 127.0.0.1:64369->127.0.0.1:64368 (ESTABLISHED)
Chromium  99433 ... TCP 127.0.0.1:64368->127.0.0.1:64369 (ESTABLISHED)
Chromium  99433 ... TCP 127.0.0.1:64368 (LISTEN)
```

Non-loopback endpoints: **none observed**. No endpoint-specific additional flag was needed. This meets the requested post-60-second `lsof` criterion; it is not a packet capture and does not prove that no short-lived connection occurred earlier in that interval or on every future launch.

A real launch inspection found that passing `"--disable-sync"` to chromiumoxide `.args()` emitted `----disable-sync`. The final builder passes bare keys. Also `.disable_default_args()` still adds `--disable-extensions` when the extensions vector is empty (confirmed in source and live process arguments). This remains on extension-free launches; S4 uses `.extension(path)` to replace it with `--load-extension`.

## S4 — PARTIAL: effective default/native messaging pass; bubble observation blocked

Fixture: `spikes/ext/` is MV3, grants `nativeMessaging`, supplies DDG `search_url`, `is_default: true`, and a stable public extension key. `spikes/native-host.py` reads a little-endian 32-bit size-prefixed JSON message and echoes one JSON reply. The public key is a throwaway extension identifier; no private key is needed to load the fixture.

Command: `SPIKE_EXTENSION="$PWD/spikes/ext" spikes/target/debug/s4`; extension loading uses builder `.extension(path)` and default args remain disabled. ID: `dmoglpnbfpfncbfhnbjokgfiecjofdcm`.

Only one manifest location was enabled per run (same profile/extension/message):

| Manifest enabled | Observed result |
| --- | --- |
| `.spike-home/profile/NativeMessagingHosts/org.iksa.spike.json` | `{"response":{"pong":{"ping":"iksa"}}}` |
| `.spike-home/Library/Application Support/Chromium/NativeMessagingHosts/org.iksa.spike.json` only | `{"error":"Specified native messaging host not found."}` |
| Neither | `{"error":"Specified native messaging host not found."}` |

The winning custom-profile manifest is restored. The real `~/Library/Application Support/Chromium/NativeMessagingHosts/` location was **not written or tested**, honoring the write restriction. Redirected HOME's alternative failed; no claim is made about real-home lookup precedence.

Default search proof is the extension-controlled persisted settings, read after browser shutdown from `.spike-home/profile/Default/Secure Preferences`, saved in `s4-search-state.json`:

```json
{
  "disable_reasons": [],
  "default_search_provider.enabled": true,
  "short_name": "Spike DuckDuckGo",
  "keyword": "spikeddg",
  "url": "https://duckduckgo.com/?q={searchTerms}"
}
```

A bounded 15 s runtime check now also succeeds: synchronous shadow-DOM traversal of `chrome://settings/searchEngines` explicitly returns **Spike DuckDuckGo (Default)**, `https://duckduckgo.com/?q=%s`, and **iksa feasibility is controlling this setting**. `s4-rendered-settings.txt` and `s4-settings.png` retain that stronger effective-default evidence. The earlier promise-based settings request did not settle and was replaced by this bounded check. No actual omnibox query was typed.

A scoped OS window screenshot filtered CoreGraphics windows to the exact Chromium executable PID; its reused-profile capture (`s4-reused-browser-window.png`) was inspected and showed the effective default with no confirmation bubble visible in that sample. A new uniquely named scoped profile was then launched with the extension/native manifest and initial UI hold to observe first-load behavior. Both capture attempts failed with the exact error:

```text
AssertionError: No on-screen scoped Chromium windows available
```

**Fresh-profile confirmation-bubble subcheck: BLOCKED** by unavailable on-screen window capture; aggregate S4 is **PARTIAL**. Native ping-pong and the effective-default runtime query succeeded on the fresh profile (`s4-fresh-reviewed.log`), but no claim is made about whether an initial install confirmation bubble appeared. An old reused-profile capture was not treated as a fresh-profile image. All captures/data stayed under `.spike-home`; no other application window was captured.

## S5 — FAIL: idle memory exceeds the target

Command: `python3 spikes/raw_probe.py s5`. Review rerun uses a newly created uniquely named scoped profile (not a reused profile). Headful browser with one visible about:blank page; no extensions. Evidence: `.spike-home/logs/s5-reviewed.log`. After 10 s idle, sum RSS of the experiment driver and its descendants. Create one hidden context/target via raw CDP, attach and navigate to Wikipedia's Rust article, settle 15 s and resample. Raw CDP is used because of S2's crate limitation.

```text
idle ... RSS_KiB=1015952
agent_navigation {... 'isDownload': False}
agent_content ... title="Rust (programming language) - Wikipedia", bytes=1380189
agent ... RSS_KiB=1322544
```

| Measurement | Summed RSS KiB | MiB |
| --- | ---: | ---: |
| Idle driver + descendants | 1,015,952 | 992.14 |
| Hidden Wikipedia agent + visible blank | 1,322,544 | 1,291.55 |
| Idle Chromium processes only | 970,608 | 947.86 |
| Chromium with hidden agent only | 1,277,120 | 1,247.19 |

`Target.getTargets` was recorded at both samples in `s5-reviewed.log`: idle has exactly one `page` at `about:blank` and two internal `browser_ui` targets (`chrome://omnibox-popup.top-chrome/` and its `omnibox_popup_aim.html` variant). Loaded inventory retains those three and adds exactly one attached `other` hidden target for the Wikipedia article in a different context. No restored ordinary tabs appeared. Internal omnibox WebUI/renderers remain part of Chromium's measured baseline.

Driver totals conservatively include Python and measurement helper/`ps` overhead. Chromium-only subtotals exclude those helpers by command path, and independently exceed ~300 MB idle. RSS sums count shared pages more than once and are not physical footprint, but RSS over the whole process tree is the requested metric. The 1,380,189 figure is serialized DOM string length; it demonstrates a real article loaded, not transfer size. Five tabs/three agents were not measured. No efficiency claim can be made from the target specification alone.

## S6 — PASS: both modes expose Turnstile markers

Command: `python3 spikes/raw_probe.py s6`; final `.spike-home/logs/s6-final.log`.
Visible control: create a normal target at `https://nowsecure.nl`, keep only the
browser-level CDP session for 30 s (no target discovery/autoattach), then attach
only to observe its DOM. Hidden agent: create hidden about:blank in the isolated
context, attach Page/Network, navigate to nowsecure.nl, settle 30 s. No stealth,
challenge click, solver, injected anti-detection code, or evasion was used.

```text
visible: title=nowsecure.nl bytes=179797 Just_a_moment=false turnstile=true hcaptcha=false recaptcha=false
hidden:  title=nowsecure.nl bytes=179625 Just_a_moment=false turnstile=true hcaptcha=false recaptcha=false
```

Both DOMs contain `https://challenges.cloudflare.com/turnstile/v0/api.js` and
`class="cf-turnstile"`. Under Task 5 first-match rules these classify as
**Turnstile**, so M1 should pause for handoff in either mode. Neither showed a
`Just a moment` interstitial, and no `cf-mitigated: challenge` response header
was captured. This is widget presence, not proof of a completed challenge or a
site-wide access block.

After the visible initial browser-only observation, an explicit reload with
Network enabled captured top-level `https://nowsecure.nl/` status **200** with
`server: cloudflare`. Those headers apply to the attached reload, not the
unobserved initial control response. The preattached hidden navigation and its
reload both captured top-level **200**, `server: cloudflare`. Challenge-platform
iframe responses were also 200. Saved DOMs are `s6-visible.html` and
`s6-hidden.html` under the ignored log directory. Normal content loading and a
Turnstile detector match can coexist; neither run establishes anti-bot evasion
or a success rate. Hand-off load must be measured on real workloads.

## Required plan changes / go-no-go

- Stop before Task 2+: the idle memory target fails and hidden Page integration is unresolved. Options for review: revise the browser/pin/target budget, or investigate an explicitly scoped follow-up spike; do not silently relax the target.
- Task 8 must translate shell-form `--flags` into chromiumoxide argument keys without `--`, and explicitly support hidden targets whose CDP type is `other` (adapter/upstream patch/dependency decision). A visible fallback changes the no-window requirement and needs a decision.
- M3 must use `.extension(path)`; `.disable_default_args()` alone still emits `--disable-extensions` in chromiumoxide 0.9.1.
- M3 cannot rely on hidden-target screencasts on this pin. A new visible target in the same context preserves cookies, but loses DOM/heap/form/challenge-local state. `activateTarget` does not reveal the existing hidden page.
- macOS throwaway runs require `--use-mock-keychain` to avoid the user-observed missing-keychain modal. Its fixed key is appropriate for these disposable profiles; production keychain handling needs its own decision.
- Register native hosts under `<user-data-dir>/NativeMessagingHosts/`; keep the profile 0700. No real-home host installation was performed.

The plan's Task 1 findings section records these decisions; Tasks 2+ remain unstarted. The failure is a go/no-go blocker, not permission to implement the rest of the plan.

## Pins and final launch contract

- Release: **156.0.8078.47-1.1**, macOS arm64; browser product Chrome/156.0.8078.47.
- [Release page](https://github.com/ungoogled-software/ungoogled-chromium-macos/releases/tag/156.0.8078.47-1.1).
- URL: `https://github.com/ungoogled-software/ungoogled-chromium-macos/releases/download/156.0.8078.47-1.1/ungoogled-chromium_156.0.8078.47-1.1_arm64-macos.dmg`
- SHA-256: `5439d292078e260928d202145d0a20f19f227424888e84beb46973c16d40a186`.
- Native manifest: `<user-data-dir>/NativeMessagingHosts/org.iksa.spike.json`, tested at `.spike-home/profile/NativeMessagingHosts/org.iksa.spike.json`.

Final explicit privacy/runtime argument list (shell form; Rust `.args()` takes these **without** leading `--`):

```text
--user-data-dir=<0700 profile under .spike-home>
--remote-debugging-address=127.0.0.1
--remote-debugging-port=0
--disable-background-networking
--disable-component-update
--disable-sync
--no-pings
--disable-domain-reliability
--disable-field-trial-config
--disable-breakpad
--metrics-recording-only
--no-default-browser-check
--no-first-run
--safebrowsing-disable-auto-update
--use-mock-keychain
```

Extension-free Rust launch additionally emits `--disable-extensions`; extension runs instead emit `--load-extension=<repo>/spikes/ext`. Both use headful mode and no `--enable-automation`. `--use-mock-keychain` is the user-requested macOS spike workaround, not an egress finding. Native messaging adds no listening port; the existing CDP port is loopback-only.

## Spike 2 — S5'

Run date: 2026-10-28, same arm64 machine and pinned Chrome/156.0.8078.47 (`Browser.getVersion` revision `1d17a2e2ee147bb7d6ae2ab6abdb3a2510c17775`). This rerun corrects S5's metric to **summed macOS `phys_footprint`**, median of three fresh-profile runs per config. MB below is decimal: `KiB × 1024 / 1,000,000`; no MiB/MB substitution. The targets remain ≤300 MB idle and ≤~600 MB with one hidden agent.

`measure.py` snapshots the exact browser PID and descendants, excludes Python/ps/footprint helpers, and uses the identical PID set for RSS and footprint. Each `footprint -p PID -f bytes` output is retained, parsing the `phys_footprint:` auxiliary line, converting exact bytes to KiB, and summing. `-f bytes` avoids the tool's default per-process MB rounding. Access/parsing/timeout failures report BLOCKED instead of a partial total. RSS remains available for comparison, but it counts shared pages repeatedly and is not the decision metric. The earlier S5 driver-inclusive RSS is not directly comparable; these RSS samples consistently include only the browser tree. Process footprint samples are sequential, not simultaneous.

Each run launches headful with the final Spike 1 privacy args, `--use-mock-keychain`, one visible `about:blank` and no extension. It creates an empty isolated browser context, settles 10 s, records idle targets and both metrics; creates exactly one hidden target in that context, navigates to Wikipedia's Rust article, settles 15 s, records loaded targets/content and both metrics. All final runs report the real Wikipedia title and 1,380,174–1,595,673 DOM characters (live-page variation). HOME, CFFIXED_USER_HOME, TMPDIR, unique 0700 profiles, downloads and logs stay under `.spike-home`. Browsers run serially and close after sampling. This uses the existing loopback TCP spike harness; it does not implement the new Task 4 pipe client.

D uses `--disable-features=PreloadTopChromeWebUI,WebUIOmniboxPopup,WebUIOmniboxAimPopup`. Names are verified against the exact running Chromium revision: [`ui_features.cc`](https://chromium.googlesource.com/chromium/src/+/1d17a2e2ee147bb7d6ae2ab6abdb3a2510c17775/chrome/browser/ui/ui_features.cc) defines `kPreloadTopChromeWebUI` enabled by default (line 205); [`webui_contents_preload_manager.cc`](https://chromium.googlesource.com/chromium/src/+/1d17a2e2ee147bb7d6ae2ab6abdb3a2510c17775/chrome/browser/ui/webui/top_chrome/webui_contents_preload_manager.cc) checks it (line 95); [`omnibox_next_features.cc`](https://chromium.googlesource.com/chromium/src/+/1d17a2e2ee147bb7d6ae2ab6abdb3a2510c17775/chrome/browser/ui/omnibox/omnibox_next_features.cc) defines the two popup features enabled by default (lines 48/52). Saved sources and URL manifest are in `.spike-home/downloads/`. D is a combined lever; this experiment does not attribute the savings separately to its three features. A/B/C idle inventories retain both `chrome://omnibox-popup.top-chrome/` and `.../omnibox_popup_aim.html`; D/E inventories contain only `about:blank`, corroborating removal of both idle WebUI targets.

Reproduce from the repository root (each command performs three serial fresh-profile runs; arguments are bare keys):

```sh
spike_s5() {
  env HOME="$PWD/.spike-home" CFFIXED_USER_HOME="$PWD/.spike-home" \
    TMPDIR="$PWD/.spike-home/tmp" SPIKE_CONFIG="$1" SPIKE_ARGS="$2" \
    python3 spikes/raw_probe.py s5
}
spike_s5 A ''
spike_s5 B 'renderer-process-limit=2'
spike_s5 C 'process-per-site'
spike_s5 D 'disable-features=PreloadTopChromeWebUI,WebUIOmniboxPopup,WebUIOmniboxAimPopup'
spike_s5 E 'renderer-process-limit=2 process-per-site disable-features=PreloadTopChromeWebUI,WebUIOmniboxPopup,WebUIOmniboxAimPopup'
```

Evidence: `.spike-home/logs/s5-<A–E>-<1–3>.log`, plus `s5-<config>-summary.log` and separate `s5-<config>-<run>-browser.log`. Each run retains flags/profile, version, target inventories, content proof, `PROCESS` ancestry/RSS, full per-PID footprint output, `PID_FOOTPRINT_KiB=`, aggregate `idle FOOTPRINT_KiB=` and `agent FOOTPRINT_KiB=`, and idle/agent `RSS_KiB=`. An initial default-formatted A1 was replaced with a new exact-byte run before computing this final matrix; all final 15 logs use bytes. A summary was regenerated from those final logs.

| Config / extra flags | Idle median footprint MB | Loaded median footprint MB | Idle median RSS MB |
| --- | ---: | ---: | ---: |
| A baseline | 398.53 | 557.79 | 988.32 |
| B `renderer-process-limit=2` | 380.43 | 549.45 | 882.29 |
| C `process-per-site` | 402.74 | 557.28 | 990.81 |
| D disable the three WebUI features above | 223.17 | 385.36 | 774.86 |
| E B + C + D | 224.11 | 366.14 | 777.58 |

Raw run evidence below gives the aggregate KiB samples in run order 1/2/3. Median footprint columns trace to `FOOTPRINT_KiB=`; idle RSS traces to its companion `RSS_KiB=` over the same PIDs.

| Config | Idle footprint KiB, runs 1/2/3 | Loaded footprint KiB, runs 1/2/3 | Idle RSS KiB, runs 1/2/3 |
| --- | --- | --- | --- |
| A | 394376.859375, 389192.671875, 388984.6484375 | 548234.359375, 543898.125, 544714.1484375 | 970112, 963152, 965152 |
| B | 371591.640625, 371511.5, 365687.453125 | 536569.140625, 586121.0234375, 507960.9296875 | 865136, 861408, 861616 |
| C | 393304.6953125, 395688.8828125, 388152.765625 | 549530.171875, 531194.3359375, 544218.171875 | 967584, 970624, 960624 |
| D | 218855.359375, 217527.1953125, 217943.21875 | 377832.859375, 372920.6953125, 376328.71875 | 760496, 751328, 756704 |
| E | 220919.3125, 218567.40625, 218855.03125 | 357559.59375, 359655.6640625, 343111.5234375 | 757984, 759664, 759360 |

**PASS:** D and E each meet both unchanged targets (≤300 MB idle and ≤~600 MB loaded). A/B/C fail the idle target. D alone is sufficient; E has the lowest loaded median in this matrix. No target relaxation is needed for this one-agent workload. This is not evidence for five tabs/three agents, nor a production rollout of these flags. All 15 samples completed without a measurement blocker; the final browser processes were closed.

Validation: Python syntax compiled; audit of all 15 logs verified 15 distinct 0700 profiles, pinned version/revision, real Wikipedia content, exact per-PID byte parsing and sums (204 PID samples), browser-only PID sets, target inventories, and both omitted WebUI URLs for all D/E idle and loaded samples. Audit summary: `.spike-home/logs/s5-audit.json`.
