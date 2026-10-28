# Brief 1 — Feasibility spikes (plan Task 1)

You are implementing **Task 1** of `docs/superpowers/plans/2026-10-28-agentic-browser-m1-agent-core.md`. Read that plan (Task 1 + Global Constraints + Task 8's `chrome_args` list) and the PRD `gemini-code-1793212240267.md` (§1.1, §1.2, §3.5, §4.4) first.

Goal: answer six yes/no questions with evidence, so later tasks don't build on a false assumption. This is throwaway code. Do NOT start Task 2+ (no crate scaffold, no renaming docs).

## Setup
- `cd /Users/srujanpendurthi/Projects/Current/iksa && git init` (iksa becomes its own repo; it is currently untracked inside the parent `Projects/Current` repo — do not touch the parent repo).
- `.gitignore`: `/spikes/target`, `/.spike-home`.
- All downloads, Chromium profiles, and data go under `iksa/.spike-home/`. Do not write anywhere else on disk except cargo's registry cache.
- Scratch cargo project: `spikes/` with `chromiumoxide = "0.9"`, `tokio` (full), `futures`, `anyhow`. One binary per spike (`src/bin/s1.rs` …) is fine.
- Machine: macOS (Darwin 27), Apple silicon presumably — check `uname -m`. rustc 1.97.

## Spikes (details in plan Task 1)
- **S1** ungoogled-chromium: latest macOS release for this arch from the `ungoogled-software/ungoogled-chromium-macos` GitHub releases. Verify SHA-256 against the published hash, mount dmg (`hdiutil attach -nobrowse`), copy app into `.spike-home/chromium/`, detach, `xattr -dr com.apple.quarantine`. Launch via chromiumoxide with `BrowserConfig::builder().disable_default_args()`, explicit args, `chrome_executable`, `user_data_dir(.spike-home/profile)`, headful (`with_head()`). Record version, URL, SHA-256 (Task 9 pins these).
- **S2** hidden targets: create a browser context, then `CreateTargetParams` with `hidden: Some(true)` + `browser_context_id`. Check navigate / `content()` / click / type on a local page; confirm no visible window (CDP `Browser.getWindowForTarget` should fail or report nothing; also `osascript`/screenshot check if easy). Try `Page.startScreencast` on it — frames or not? If not, find the cheapest way to make that agent page visible for a human hand-off (e.g. `Target.activateTarget`, new visible target in same context keeping cookies) and record what works.
- **S3** egress: privacy args from plan Task 8 list. Idle 60 s on `about:blank`, then `lsof -nP -i` filtered to the Chromium process tree (`pgrep -f .spike-home`). Report every non-loopback endpoint; for each, find a flag/pref that stops it and re-test. Final arg list goes in the report.
- **S4** extension + native messaging: write a minimal MV3 extension in `spikes/ext/` with `chrome_settings_overrides.search_provider` (DuckDuckGo, `is_default: true`) and `nativeMessaging`; load with `--load-extension=` (note: chromiumoxide adds `--disable-extensions` when its extension list is empty unless default args are disabled — you are disabling them). Native host = tiny script or Rust bin echoing one JSON message. Find which directory this Chromium reads host manifests from with a custom `--user-data-dir` (try `<user-data-dir>/NativeMessagingHosts/` and `~/Library/Application Support/Chromium/NativeMessagingHosts/`). Check: does an omnibox search go to DDG (navigate via typing into omnibox is hard over CDP — acceptable proof: read `chrome://settings/searchEngines` state or the extension-controlled default via a screenshot/`Page.navigate` to the search keyword); does the ping-pong work; any confirmation bubble.
- **S5** memory: headful idle + one hidden agent context on a mid-size page (e.g. `https://en.wikipedia.org/wiki/Rust_(programming_language)`). Sum RSS over the whole process tree, idle and with the agent page. Compare to 300 MB idle target.
- **S6** bot visibility: load `https://nowsecure.nl` (a) in a normal visible tab, (b) in a hidden agent context. Record whether each passes or shows a Cloudflare challenge (use the challenge rules in plan Task 5). Observation only — no stealth/evasion work.

## Output
- `docs/spikes.md`: one section per spike — **PASS / FAIL / PARTIAL**, commands run, evidence (log excerpts, numbers), and any plan change it implies (e.g. "S2: hidden targets don't screencast; M3 hand-off must use X"). End with: pinned ungoogled-chromium version/URL/SHA-256, final privacy arg list, native host manifest path.
- Commit only `spikes/`, `docs/spikes.md`, `.gitignore`, `docs/briefs/`, `docs/superpowers/` with message `docs: feasibility spike results`. Do not commit `.spike-home/` or the gemini-*.md files.
- Last message: a short summary table of the six results plus anything that should change in the plan.

If a spike is blocked (e.g. download fails, sandbox denies launching), do not fake it — mark BLOCKED with the exact error and move on to the next.
