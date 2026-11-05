# iksa — M4 Providers, Vision, Update, MCP, Extensions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish v0.1.0-alpha's remaining PRD items, except the research context system:
- OpenAI-compatible and local model providers, with DeepSeek as an explicit opt-in;
- screenshots for vision models;
- `iksa update`;
- an MCP server for other AI tools;
- user-installed MV3 extensions, with uBlock Origin Lite and Bitwarden checked.

**Architecture:**
- **Providers:** `llm.rs` gains a second wire format (OpenAI Chat Completions), chosen by `model.provider`. Ollama, llama.cpp's server, OpenAI and DeepSeek all speak it, so one client covers them. The planner, gates and audit are unchanged.
- **`iksa update`:** reads the newest arm64 release from GitHub's API, verifies its SHA-256 against the asset's published digest and swaps it in with a launch check and rollback.
- **`iksa mcp`:** a stdio MCP server. It reads the ledger, queues goals, and reads pages through the daemon's M3 control socket. It can never approve anything.

**Tech Stack:** M3 stack. No new crates; the MCP JSON-RPC is hand-written over `serde_json`. `curl`/`hdiutil` for update, as `setup` already uses.

**Spec:**
- `PRD.md` §1.2.2 (update), §3.2 (planner providers, vision rule), §3.6 (MCP, extensions), §3.7 (model config, DeepSeek warning).
- `reports/DeepSeek Super Agent for iksa.md` (local, untracked: DeepSeek API facts).
- M3 plan `docs/superpowers/plans/2026-11-04-m3-human-browsing.md` (control socket, extension loading).

## Scope note: research context system is not in this plan

PRD §3.6's research context system (local ColBERT/ONNX vectorization plus graph storage) is a separate subsystem:
- an ONNX runtime would likely exceed the 60 MB binary limit and add the project's first native ML dependency;
- it needs its own design (what gets indexed, retention, how the planner uses it).

**Recommendation for its own brainstorm:** start with SQLite FTS5 (already compiled into `rusqlite` `bundled`) over committed task outputs and extracted text, with `never_send` and `retention_days` honoured. Add embeddings only if lexical search measurably falls short. That plan is written after a design conversation with the user.

## Decisions this plan makes

- **Provider values:** `anthropic` (existing) and `openai` (Chat Completions at `{endpoint}/v1/chat/completions`). Ollama is `endpoint: "http://127.0.0.1:11434"`, llama.cpp's server is `http://127.0.0.1:8080`, DeepSeek is `https://api.deepseek.com`.
  - DeepSeek uses the OpenAI-compatible path, **not** DeepSeek's Anthropic-compatible endpoint, which silently remaps `claude-*` names (PRD §3.7).
  - If `provider: "anthropic"` is configured with an `api.deepseek.com` endpoint, config load rejects it with `use provider "openai" for DeepSeek`.
- **OpenAI request body:**
  - `model`, `max_tokens: 4096`;
  - `messages: [{role:"system",content:SYSTEM},{role:"user",content:<text or parts>}]`;
  - `tools: [{type:"function",function:{name,description,parameters,strict:true}}]`;
  - `tool_choice: "auto"`, `parallel_tool_calls: false`.
  - No `thinking`, `output_config`, `fallbacks` or `cache_control`. `effort` is ignored for `openai` (documented).
- **OpenAI reply parsing:**
  - The tool call is `choices[0].message.tool_calls[0].function`, whose `arguments` is a JSON **string**, parsed into a `Value`.
  - `message.refusal` (non-null) means `Err("model refused: …")`, matching Anthropic's refusal behaviour.
  - `finish_reason: "length"` means `Err("model output truncated")`.
  - Unparseable `arguments` are returned as `Reply { tool: Some((name, Value::Null)), .. }`, so the planner's invalid-action path feeds the error back and counts toward the 3-strike limit.
  - Retries (429/5xx/529, `retry-after` capped at 30 s, 3 attempts) and no redirects stay shared with Anthropic.
- **Transport rule:** `http://` endpoints are allowed only for loopback hosts (`127.0.0.1`, `::1`, `localhost`). Any other host needs `https://`. Config load enforces this and names the endpoint.
- **API keys:**
  - `anthropic`: unchanged (`ANTHROPIC_API_KEY`, else Keychain `iksa`/`anthropic`).
  - `openai`: `OPENAI_API_KEY`, else Keychain `iksa`/`<endpoint host>`.
  - A loopback endpoint needs no key and sends none.
  - `iksa key set [host]` with no argument keeps today's Anthropic behaviour.
- **DeepSeek opt-in:**
  - If the endpoint host is `deepseek.com` or one of its subdomains, config load requires `model.accept_data_terms: "deepseek"`. Otherwise it fails with: `DeepSeek stores API data in the PRC and its terms permit training on submitted data. To use it anyway, set "accept_data_terms": "deepseek" in model.`
  - No other provider uses the field, and it is rejected if set for a non-DeepSeek endpoint (catches typos).
- **Vision:** new `model.vision: bool` (default false).
  - When true, each planner request adds one screenshot of the 1280×800 viewport: `Page.captureScreenshot {format:"jpeg", quality:50}`. Anthropic gets an `image` block (base64); OpenAI gets an `image_url` data URL.
  - **Field values must not leak.** Before capture, inject a style that masks values: inputs, textareas and contenteditables get `color:transparent; text-shadow:none; caret-color:transparent`, and selects are covered too. The style is removed right after capture.
  - **No screenshot** while a secret is on the page (the existing snapshot rule) and never for `never_send` (the request is never made).
  - The screenshot is part of the stored request body, so the audit keeps exactly what was sent. Storage per vision task is measured separately in `docs/perf.md`; the 500 KB target applies to non-vision tasks.
- **`iksa update`:**
  - `GET https://api.github.com/repos/ungoogled-software/ungoogled-chromium-macos/releases/latest`. Pick the single asset matching `_arm64-macos.dmg` and take its `digest` (`sha256:<hex>`).
  - Missing asset or digest → `Err`, and nothing changes.
  - If the tag is not newer than the installed version, print `up to date (<version>)`.
  - Otherwise download to `chromium/download.dmg` with `curl -fL`, verify, install to `chromium.new/`, then launch it once (`Browser.getVersion` within 15 s, product must start with `Chrome/<major>`).
  - On success, swap: `chromium` → `chromium.old`, `chromium.new` → `chromium`, write the `version` marker, delete `chromium.old`. On any failure, delete `chromium.new` and keep the old build.
  - `update --check` only prints `available: <tag>` or `up to date`.
  - The installed version may now differ from `setup::PIN`. `setup` keeps installing `PIN` only when nothing is installed.
  - The PRD trusts GitHub for authenticity, and this plan doesn't change that. The digest check protects only integrity.
- **MCP (`iksa mcp`, stdio, JSON-RPC 2.0):**
  - Handles `initialize` (replies with the client's `protocolVersion` if it is in `SUPPORTED_VERSIONS`, else the newest in that list), `notifications/initialized`, `ping`, `tools/list` and `tools/call`.
  - `SUPPORTED_VERSIONS`: read the current MCP specification at implementation time and record the versions and source URL in the commit message.
  - Tools:
    - `list_tasks {}`;
    - `task_report {id}` (Markdown);
    - `inbox {}`;
    - `ask {goal}` (queues; requires a configured model; gates apply as usual);
    - `read_page {url}` (via the control socket op `read_page`: a fresh hidden context, navigate, observe, return `observe::render` text, dispose; Read risk only; `never_send` hosts → tool error `blocked: <host> is on never_send`).
  - **Deliberately absent:** approve, resume, accept, dismiss, grant, pause, schedule and any write action. Another AI must never be able to sign off on iksa's gates.
- **User extensions:** new config `extensions: Vec<String>` holds absolute paths to unpacked MV3 extension directories. They are appended to `--load-extension` after iksa's own and apply to **human browsing only**; agent contexts are created without them (the check is part of Task 4's test).
  - iksa never downloads extensions (the Chrome Web Store is a Google service). The README tells users to unpack releases themselves.
  - uBlock Origin is checked as **uBlock Origin Lite** (MV3). Classic uBlock Origin is MV2 and is not targeted.

## Global Constraints

- All M1–M3 constraints hold: binary < 60 MB, memory and egress pillars, no TCP/UDP listeners, 0700 data dir, ignored tests with `--skip live_`.
- **No `live_*` test without the user's go-ahead**, and none for local models without the user starting the local server.
- Tests use loopback mocks for every provider and for the GitHub API (endpoint override `IKSA_UPDATE_API`, read only when `IKSA_MOCK_KEYCHAIN=1`, the existing test-mode switch).
- `iksa` itself adds exactly these outbound hosts: the configured model endpoint, `api.github.com`, and the release asset download host (`github.com` redirecting to `objects.githubusercontent.com`; `curl -L` follows these for `update` only; the model client still never follows redirects).

## Review Focus

- **A small local model returns broken tool arguments (common with 7B models):** the step is fed back as an invalid action; 3 in a row → `Failed`. Test `openai_bad_arguments_fed_back` (Task 0).
- **A user types `http://` for a remote endpoint:** config error naming the endpoint; no request leaves. Test `http_only_on_loopback` (Task 0).
- **Update interrupted, digest mismatch, or new build fails to launch:** the old Chromium stays installed and working. Tests `update_digest_mismatch_keeps_old`, `update_launch_failure_rolls_back` (Task 3).
- **An MCP client tries to approve its own queued task:** no such tool; `tools/call approve` → JSON-RPC error `unknown tool`. Test `mcp_cannot_approve` (Task 4).
- **Vision on a page with a typed value or a secret:** the screenshot hides values, and none is taken while a secret is on the page. Tests `screenshot_masks_field_values`, `no_screenshot_with_secret_on_page` (Task 2).

---

### Task 0: OpenAI-compatible provider (OpenAI, Ollama, llama.cpp)

**Files:** `src/llm.rs`, `src/config.rs`, `src/engine.rs` (key lookup call site), `src/main.rs` (`key set [host]`), `tests/planner.rs`.

**Interfaces:**
- Produces:
  - `ModelConfig.provider` validated to `"anthropic" | "openai"`.
  - `llm::request_body(cfg, system, tools, user: &[Part]) -> Value`, which dispatches on provider. `pub enum Part { Text(String), Jpeg(String /*base64*/) }`. M2a callers pass `&[Part::Text(user)]`.
  - `llm::call(cfg, key: Option<&str>, body)` returns the same `Reply` for both providers. `api_key(cfg) -> anyhow::Result<Option<String>>` returns `None` for loopback endpoints.
  - `pub fn is_loopback(endpoint: &str) -> bool` in `config.rs`.
- [ ] **Step 1: Failing tests:**
  - Unit `openai_request_shape`: tool `function.strict == true`, `parallel_tool_calls == false`, first message has role `system`, and no `thinking` / `output_config` / `fallbacks` keys.
  - Unit (mock server) `openai_parses_tool_call`: `arguments` `"{\"url\":\"https://a.test\"}"` gives `tool == ("navigate", {"url":…})`.
  - `openai_refusal_is_error`.
  - `openai_truncation_is_error`.
  - `openai_retries_on_429`.
  - `loopback_sends_no_key`: the mock records no `authorization` header.
  - Unit `http_only_on_loopback`: `http://127.0.0.1:11434` and `http://localhost:8080` are OK; `http://10.0.0.5:11434` and `http://api.openai.com` → `Err` containing the endpoint.
  - `#[ignore]` `openai_bad_arguments_fed_back`: the mock returns `arguments: "{not json"` three times → `Failed("model kept choosing invalid actions")`. Each request after the first includes the error in its step log.
  - `#[ignore]` `openai_goal_commits`: the M2a mock-model browser flow (navigate → extract → done) over the OpenAI wire format → `Committed`.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** `cargo test` + full ignored suite. **Step 5:** Commit `feat: OpenAI-compatible model provider (OpenAI, Ollama, llama.cpp)`.

### Task 1: DeepSeek opt-in

**Files:** `src/config.rs`, `tests/cli.rs`.

**Interfaces:**
- Produces: `ModelConfig.accept_data_terms: Option<String>`, validated as in Decisions.
- [ ] **Step 1: Failing tests:**
  - Unit `deepseek_requires_ack`: `{provider:"openai", endpoint:"https://api.deepseek.com", name:"deepseek-chat"}` → `Err` containing `stores API data in the PRC`. Adding `accept_data_terms: "deepseek"` → OK.
  - Unit `deepseek_anthropic_endpoint_rejected`: provider `anthropic` with endpoint `https://api.deepseek.com/anthropic` → `Err` containing `use provider "openai" for DeepSeek`.
  - Unit `ack_only_for_deepseek`: `accept_data_terms` set with an OpenAI endpoint → `Err`.
  - `cli::deepseek_config_error_shown`: `list` with the unacknowledged config → exit 1 and stderr contains the PRC sentence.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** pass. **Step 5:** Commit `feat: DeepSeek only with explicit data-terms opt-in`.

### Task 2: Vision screenshots

**Files:** `src/engine.rs`, `src/config.rs` (`vision`), `src/llm.rs` (image parts), `tests/planner.rs`.

**Interfaces:**
- Consumes: `Part::Jpeg` from Task 0.
- Produces:
  - `const MASK_CSS: &str` in `engine.rs`.
  - `async fn screenshot(&self, b, session) -> anyhow::Result<String>`: injects `MASK_CSS` via `Runtime.evaluate` (a `<style id="__iksa_mask">`), captures, removes the style in all cases, and returns base64.
  - The planner adds `Part::Jpeg` only when `cfg.model.vision` and no secret has been typed on the current page.
- [ ] **Step 1: Failing tests (`#[ignore]`):**
  - `screenshot_masks_field_values`: two fixture pages are identical except `<input value="AAAAAAAA">` vs `value="ZZZZZZZZ"`. `screenshot` of each returns **byte-identical** JPEGs, and the style element is gone afterwards.
  - `vision_request_has_image`: with `vision: true`, the mock model's recorded request contains one `image` block (Anthropic) or one `image_url` part (OpenAI). With `vision: false` there is none.
  - `no_screenshot_with_secret_on_page`: a goal that types `{{secret:demo}}` → the next request on that page has no image, and the request after the following navigation does.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** full ignored suite. Record the CAS bytes for one vision task in `docs/perf.md` (`## M4 vision storage`). **Step 5:** Commit `feat: masked screenshots for vision models`.

### Task 3: `iksa update`

**Files:** `src/setup.rs`, `src/main.rs`, `tests/cli.rs`.

**Interfaces:**
- Produces:
  - `pub fn latest(api_json: &Value) -> anyhow::Result<Release>` with `Release { tag: String, url: String, sha256: String }`.
  - `pub fn newer(tag: &str, installed: &str) -> bool`, comparing dotted numeric parts then the `-N.M` suffix.
  - `pub fn update(home: &Path, check_only: bool) -> anyhow::Result<String>`, which returns the line to print.
  - `install` is refactored so its download→verify→mount→copy pipeline takes `(url, sha256, dest_dir)`, and `setup` and `update` share it.
- [ ] **Step 1: Failing tests:**
  - Unit `latest_picks_arm64_digest`: a fixture JSON with x86_64 and arm64 assets returns the arm64 URL and hex digest. With no arm64 asset → `Err`. With the asset but no `digest` → `Err` containing `digest`.
  - Unit `version_ordering`:
    - `newer("157.0.1.2-1.1", "156.0.8078.47-1.1")`;
    - `!newer("156.0.8078.47-1.1", "156.0.8078.47-1.1")`;
    - `newer("156.0.8078.47-2.1", "156.0.8078.47-1.1")`;
    - `!newer("155.9", "156.0")`.
  - `cli::update_check_up_to_date` (mock API on loopback via `IKSA_UPDATE_API`) → `up to date (156.0.8078.47-1.1)`, and nothing downloaded.
  - `cli::update_digest_mismatch_keeps_old`: the mock serves a small file whose digest doesn't match → exit 1 `checksum mismatch`. `chromium/version` is unchanged and `chromium.new` is absent.
  - `#[ignore]` `update_launch_failure_rolls_back`: the mock serves a valid DMG whose app binary exits immediately. Built in-test with `hdiutil create` from a folder holding `Chromium.app/Contents/MacOS/Chromium` = `#!/bin/sh\nexit 1`. Update → `Err` containing `launch check failed`, and the old install still launches.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** pass. **Step 5:** Commit `feat: iksa update with digest check and rollback`.

### Task 4: MCP server

**Files:** Create `src/mcp.rs`; modify `src/main.rs`, `src/control.rs` (op `read_page`), `src/daemon.rs`, `tests/mcp.rs` (new).

**Interfaces:**
- Consumes: M3 `control` socket client (`control::request(home, op: Value) -> anyhow::Result<Value>`; add it there if M3 only has the server side), `Ledger`, `audit::render`.
- Produces:
  - `pub fn serve(home: &Path, input: impl BufRead, output: impl Write) -> anyhow::Result<()>`: newline-delimited JSON-RPC as MCP stdio requires; it returns on EOF.
  - `pub const SUPPORTED_VERSIONS: &[&str]`.
  - Tool input schemas are JSON Schema objects with `additionalProperties: false`.
  - Tool results are `{content:[{type:"text",text:…}], isError?: true}`.
- [ ] **Step 1: Failing tests (`tests/mcp.rs`, driving `iksa mcp` as a child over stdio):**
  - `mcp_handshake`: `initialize` with a supported version echoes it, with `capabilities.tools` present and `serverInfo.name == "iksa"`. An unsupported version gets the newest supported one back.
  - `mcp_tools_list`: names are exactly `["ask","inbox","list_tasks","read_page","task_report"]` (sorted).
  - `mcp_cannot_approve`: `tools/call {name:"approve"}` → JSON-RPC error whose message contains `unknown tool`, and the ledger is unchanged.
  - `mcp_ask_queues_goal`: with a model configured, `ask {goal:"x"}` → text with the task id, and `list` shows `[goal] x Queued`. Without a model → `isError: true`, `configure "model"`.
  - `mcp_task_report`: the report text for an existing task. An unknown id → `isError: true`.
  - `#[ignore]` `mcp_read_page`: with the daemon running, `read_page` of a fixture URL contains `[1]` element lines. A `never_send` host → `isError` `blocked: 127.0.0.1 is on never_send`, and no hidden context remains afterwards.
  - `mcp_bad_json`: a non-JSON line → JSON-RPC parse error `-32700` reply, and the server keeps serving.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** pass. **Step 5:** Commit `feat: iksa mcp (read and queue only)`.

### Task 5: User extensions, uBlock Origin Lite and Bitwarden checks

**Files:** `src/config.rs` (`extensions`), `src/engine.rs` (`launch_args`), `tests/m3.rs`, `tests/fixtures/blocker-ext/` (new minimal MV3 DNR extension), `docs/perf.md`.

**Interfaces:**
- Produces: `Config.extensions: Vec<String>`. Each must be an absolute path to a directory containing `manifest.json` with `"manifest_version": 3`, else config error naming the path. `--load-extension=<iksa>,<user1>,…`.
- [ ] **Step 1: Failing tests:**
  - Unit `extensions_validated`: a relative path, a missing dir, an MV2 manifest, or a manifest whose `key` derives `extension::EXTENSION_ID` → `Err` naming it. (iksa's public key is in this public repo; an unpacked extension declaring it would get iksa's ID and pass the native host's `allowed_origins`.)
  - `#[ignore]` `user_extension_blocks_in_human_tab`: the fixture extension's declarativeNetRequest rule blocks `/ad.js` on the fixture host. A human tab (`browse`) loading `/page` has `window.adLoaded === undefined`, and the fixture server logged no request for `/ad.js`.
  - `#[ignore]` `agent_contexts_have_no_user_extensions`: the same page in an agent task → `/ad.js` **is** requested (the extension doesn't run in agent contexts). This pins the documented behaviour.
- [ ] **Step 2:** FAIL. **Step 3:** implement. **Step 4:** pass, plus the memory pillar with the fixture extension loaded.
- [ ] **Step 5: Manual checks for the user (Codex prepares and stops):**
  - **H3 uBlock Origin Lite:** unpack the latest release, add it to `extensions`, `browse` a news site, and confirm the toolbar count is above 0.
  - **H4 Bitwarden:** unpack the release build, log in, and autofill a test login form.

  The user reports results and Claude records them in `docs/perf.md`. Bitwarden syncing to its own server is the user's choice and is excluded from the egress pillar (which runs without user extensions).
- [ ] **Step 6:** Commit `feat: user-installed MV3 extensions in human browsing`.

### Task 6: Docs and pillars

**Files:** `README.md`, `CLAUDE.md`, `PRD.md` §6, `docs/perf.md`.

- [ ] **Step 1:** Full ignored suite, the pillar tests and `ls -l target/release/iksa` (< 60 MB). Record the results in `docs/perf.md` `## M4`.
- [ ] **Step 2: Update the README:**
  - provider examples (OpenAI, Ollama, llama.cpp; DeepSeek with the warning text verbatim);
  - `vision` and what it masks;
  - `update` / `update --check`;
  - MCP setup: a client config snippet with `"command": "/path/to/iksa", "args": ["mcp"]`, and the list of tools and what is deliberately missing;
  - `extensions` (MV3 only, user-unpacked, human browsing only).
- [ ] **Step 3:** Module map: add `mcp`. PRD §6: M4 status. Add a new PRD row: **M5 Research context**, with "needs design; FTS5 first".
- [ ] **Step 4:** Commit `docs: M4 README, PRD milestones, perf`.

---

## Done checklist

| # | Task | Check | Pass |
|---|---|---|---|
| J1 | 0 | provider unit + ignored tests | OpenAI wire format works; loopback-only http; bad args fed back |
| J2 | 1 | DeepSeek config tests | Refused without opt-in; Anthropic-compat endpoint refused |
| J3 | 2 | vision tests | Identical screenshots for different field values; no image with a secret on page |
| J4 | 3 | update tests | Check-only, digest mismatch and launch failure leave the old build working |
| J5 | 4 | MCP tests | Handshake, five tools only, cannot approve, never_send respected |
| J6 | 5 | extension tests + user H3/H4 | Human tabs get user extensions; agent contexts don't |
| J7 | 6 | pillars, binary size | All M1–M3 budgets still met; < 60 MB |
