# Product Requirement Document (PRD)

## Project Title: Agentic Browser (Codename: iksa)
**Document Status:** Draft / Engineering Architecture Stage  
**Target Release:** v0.1.0-alpha  

---

## 1. Executive Summary & Product Vision
Existing web browsers are passive document viewers. iksa is an **Agent-First Browser** with a Rust core. Agents operate autonomously in background contexts handling proactive web tasks, research, and data synthesis, while humans retain total visibility via an "Agent View", configurable approval gates, and a content-addressed audit log.

### 1.1 Product Pillars (every milestone is checked against these)
1. **Private:** No data leaves the machine unless the user asked for it. No telemetry, no Google services, no other browser profile is ever read. Model calls (local or cloud) go only to the provider the user configured, carry the minimum page content, and are logged.
2. **Compute & memory efficient:** One browser process tree for humans and agents, no second web engine, bounded agent concurrency, measured every milestone.
3. **Proactive agents:** Agents act on schedules, page visits, and watched changes, and propose tasks — not only run what is queued.
4. **Customizable:** Search provider (human address bar and agents alike), risk thresholds, concurrency, retention, and model are user settings in one config file.

### 1.2 Resolved Architecture Decisions (2026-10-28)
1. **Engine:** All web content runs in **one headful Chromium instance** driven over CDP through iksa's own client on `--remote-debugging-pipe` (no TCP port; chromiumoxide dropped — it cannot drive hidden targets). Launched with `--disable-features=PreloadTopChromeWebUI,WebUIOmniboxPopup,WebUIOmniboxAimPopup` (idle footprint 398 → 223 MB in Spike 2). Human tabs are normal windows; agent tasks run in isolated browser contexts whose targets are created `hidden` (never painted to a window). There is no Wry/WKWebView.
2. **Chromium source:** A pinned **ungoogled-chromium** build (Google services stripped at compile time) is downloaded, SHA-256 verified, to `~/.iksa/chromium/` by `iksa setup`; `iksa update` moves the pin forward so security patches are not missed. Not bundled (keeps the core binary < 60 MB); never the user's installed Chrome.
3. **iksa UI = a bundled MV3 extension** loaded into that Chromium: side panel (task ledger, suggestion inbox, approvals), Agent View page (CDP `Page.startScreencast` + `Input.dispatch*Event`), and `chrome_settings_overrides.search_provider` for the address-bar search engine. It talks to the iksa daemon over **native messaging** (stdio) — no extra listening port.
4. **Session sharing:** Humans browse in iksa's own Chromium profile. Agent contexts receive copy-on-write cookie copies (`Storage.getCookies` / `Storage.setCookies`); changed cookies sync back on `Committed`. iksa never reads or decrypts other browser profiles.
5. **Memory targets** are measured over the whole process tree (iksa + Chromium); see §5.

---

## 2. Target Persona & User Stories
* **Power Users & Knowledge Workers:** Delegate multi-tab research, flight tracking, and automated form execution to background workers.
* **Developers & AI Engineers:** Tap into native MCP endpoints and modular local model runtimes.

### Core User Stories
1. *Proactive Task Delegation:* Queue tasks, schedule them, or accept agent-suggested tasks; agents fulfill them in the background.
2. *Agent Inspection:* Toggle into "Agent View" to watch the live agent page and tool calls.
3. *Immutable Content-Addressed Audit Trail:* View versioned action changesets with near-zero storage bloat.
4. *Swappable Search & Gates:* Choose search providers (Kagi, DuckDuckGo, Brave, SearXNG, custom URL template) used by both the address bar and agents; adjust risk-based approval gates.

---

## 3. Key Feature Specifications

### 3.1 Background Execution & Session Sandboxing
* **Hidden agent contexts:** Isolated browser contexts with hidden targets; no window paint.
* **Session Cookie Sandbox:** Agents inherit authenticated state from the human's iksa profile via copy-on-write cookie copies (§1.2.4), bypassing basic anti-bot blocks without credential leakage.

### 3.2 Task Ledger & Proactive Engine
* Task ledger with lifecycle states `Suggested` -> `Queued` -> `Executing` -> `Awaiting Human Hand-off` -> `Committed`, plus terminal `Failed` and `Dismissed`.
* **Triggers:** schedule (interval/cron), page visit (URL pattern seen in a human tab), watch (a selector's content changes on a URL).
* **Planner:** turns a natural-language goal into validated actions, step by step, using whichever model the user configured — cloud (e.g. Anthropic, OpenAI-compatible APIs) or local (Ollama / llama.cpp). No model is configured by default; proactive features stay off until one is.
* **Page representation:** the model sees a pruned accessibility tree with numbered element refs (not raw HTML), and acts on refs rather than hand-written CSS selectors, so actions survive markup changes. Screenshots are sent only if the configured model supports vision and the user enabled it.
* **What leaves the machine:** form field values are never sent; secret fields are redacted; domains on `never_send` (default: risk-level-2 / banking domains the user marks) are never sent to a model; every model request body is stored in the audit log so the user can see exactly what was shared.
* **Suggestion inbox:** heuristic page-context matching and triggers create `Suggested` tasks; the user accepts or dismisses.

### 3.3 Security & Approval Gates
* Configurable risk thresholds: Level 0 (read-only, fully autonomous) to Level 2 (financial transactions, mandatory human sign-off; no setting can auto-approve Level 2).

### 3.4 Content-Addressable Store (CAS) Action Audit Log
* **Deduplication via CAS:** DOM snapshots are split into content-defined chunks, hashed (BLAKE3), stored as immutable zstd blobs. Changesets record only pointers.
* Final tasks compile to human-readable Markdown audit reports.
* **Privacy of the log:** data dir is owner-only (0700); typed secrets are redacted; Level-2 steps store no DOM snapshot; records older than `retention_days` are garbage-collected.

### 3.5 Privacy Baseline
* Chromium launched with background networking, component updates, sync, pings, domain reliability, field trials, crash upload and Safe Browsing pings disabled. Verified by an automated egress test: an idle browser makes zero non-loopback connections.
* CDP over a pipe, never a TCP port; the profile directory is 0700.
* The only network calls iksa itself makes: `iksa setup` / `iksa update` (download the pinned build), and model calls the user configured.

### 3.6 Extension & Integration Ecosystem
* **MCP Native Host:** Expose the ledger and browser actions as MCP tools; connect to any user-configured model, local or cloud.
* **Extensions:** MV3 extensions run natively in Chromium (no shim); load unpacked via CDP `Extensions.loadUnpacked`. v0.1 targets uBlock Origin and Bitwarden.
* **Research Context System:** Local vectorization (ColBERT/ONNX on blocking worker pools) and hierarchical graph storage, all local.

### 3.7 Configuration
* One file, `~/.iksa/config.json`: `search` (active provider + provider list of `{name, url_template}` with `{q}`), `auto_max` risk level, `max_agents`, `retention_days`, `model` (provider, endpoint, model name, API-key reference stored in the OS keychain — never in the file; `vision` on/off), `never_send` (domain list).

---

## 4. Technical Challenges & Architecture Open Items

### 4.1 Anti-Bot & CAPTCHA Wall (Human Intervention Hand-off)
* **Challenge:** Agents will hit interactive challenges (Cloudflare Turnstile, hCaptcha, reCAPTCHA).
* **Strategy:** On detection (DOM markers or HTTP 403/503 with Cloudflare fingerprint) execution pauses in `Awaiting Human Hand-off`. The browser raises an urgent alert; the user opens Agent View, solves the challenge in the agent's own context, clicks "Resume Agent", and the background worker continues.

### 4.2 Human/Agent Race Conditions
* **Challenge:** State corruption if a human and an agent mutate the same site concurrently.
* **Strategy:** Agents work in isolated browser contexts (copy-on-write). A per-domain **mutation lock** serializes agent write-steps on domains open in human tabs; cookie diffs sync back only on `Committed`.

### 4.3 Extensions
* **Resolved:** Web content runs in Chromium (§1.2.1), so MV3 extensions run natively; no API emulation.

### 4.4 CDP Transport
* **Resolved:** iksa's own CDP client uses `--remote-debugging-pipe` (fd 3/4); no debugging port exists.

---

## 5. Technical Constraints & Non-Functional Requirements

| Metric | Target Requirement |
| :--- | :--- |
| **Binary Footprint** | < 60 MB (iksa core; Chromium downloaded separately) |
| **Agent Action Latency** | < 25ms overhead per DOM interaction step |
| **Storage Overhead** | CAS deduplication (< 500 KB per completed task) |
| **Memory Footprint** | Summed macOS `phys_footprint` over the whole process tree (iksa + Chromium), median of 3 runs: ≤ 300 MB idle; ≤ 1.2 GB for 5 tabs + 3 active background agents |
| **Privacy** | Zero non-loopback connections from an idle browser; no telemetry |
| **Concurrency** | At most `max_agents` (default 3) agent contexts at once |
