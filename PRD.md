# Product Requirement Document (PRD)

## Project Title: Agentic Browser (Codename: iksa)
**Document Status:** Engineering — M1 done, M2a code done (live eval not run), M2b, M3 and M4 planned (updated 2026-11-04)  
**Target Release:** v0.1.0-alpha  
**License:** MIT

---

## 1. Executive Summary & Product Vision
Existing web browsers are passive document viewers. iksa is an **Agent-First Browser** with a Rust core. Agents operate autonomously in background contexts handling proactive web tasks, research, and data synthesis, while humans retain total visibility via an "Agent View", configurable approval gates, and a content-addressed audit log. iksa asks for human attention as rarely and as precisely as safety allows.

### 1.1 Product Pillars (every milestone is checked against these)
1. **Private:** No data leaves the machine unless the user asked for it. No telemetry, no Google services, no other browser profile is ever read. Model calls (local or cloud) go only to the provider the user configured, carry the minimum page content, and are logged.
2. **Compute & memory efficient:** One browser process tree for humans and agents, no second web engine, bounded agent concurrency, measured every milestone.
3. **Proactive agents:** Agents act on schedules, page visits, and watched changes, and propose tasks — not only run what is queued.
4. **Customizable:** Search provider (human address bar and agents alike), risk thresholds, concurrency, retention, and model are user settings in one config file.

### 1.2 Resolved Architecture Decisions
1. **Engine (2026-10-28):** All web content runs in **one headful Chromium instance** driven over CDP through iksa's own client on `--remote-debugging-pipe` (no TCP port; chromiumoxide dropped — it cannot drive hidden targets). Launched with `--disable-features=PreloadTopChromeWebUI,WebUIOmniboxPopup,WebUIOmniboxAimPopup` (idle footprint 398 → 223 MB in Spike 2). Human tabs are normal windows; agent tasks run in isolated browser contexts whose targets are created `hidden` (never painted to a window). There is no Wry/WKWebView.
2. **Chromium source (2026-10-28):** A pinned **ungoogled-chromium** build (Google services stripped at compile time) is downloaded, SHA-256 verified, to `~/.iksa/chromium/` by `iksa setup`; `iksa update` moves the pin forward so security patches are not missed. Not bundled (keeps the core binary < 60 MB); never the user's installed Chrome.
3. **iksa UI = a bundled MV3 extension (2026-10-28, refined 2026-11-04):** loaded into that Chromium with a side panel (task ledger, suggestion inbox, approvals), an Agent View page (CDP `Page.startScreencast` + `Input.dispatch*Event`), and `chrome_settings_overrides.search_provider` for the address-bar search engine. The extension files are embedded in the iksa binary and written to the data dir at launch with a manifest generated from `config.json`. The extension talks to iksa over **native messaging** (stdio). The native host relays to the daemon over an owner-only Unix socket (`~/.iksa/daemon.sock`, mode 0600) — a file, not a network port; iksa never listens on TCP or UDP. The extension has no host permissions and makes no network requests.
4. **Session sharing (2026-10-28, refined 2026-11-04):** Humans browse in iksa's own Chromium profile. Agent contexts receive copy-on-write cookie copies (`Storage.getCookies` / `Storage.setCookies`) **only for domains the user lists in `share_cookies`** (default: none), because logged-in pages carry account data to the model. Changed cookies sync back only on `Committed`; if the human changed the same cookie meanwhile, the human's value wins. iksa never reads or decrypts other browser profiles.
5. **Memory targets (2026-10-28)** are measured over the whole process tree (iksa + Chromium); see §5.
6. **One driver process (2026-11-04):** `iksa daemon` is the only process that drives Chromium; `iksa run` is the same loop in drain mode. Both take one run lock per data dir. Chromium starts lazily and closes after 60 s with no running task and no human tab, so an idle daemon holds no browser.
7. **Model API (2026-11-03):** Anthropic Messages first, over raw HTTPS (`ureq`), API key from `ANTHROPIC_API_KEY` or the macOS Keychain — never in the config file or process arguments. Other providers are M4 (§6).

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
* **Session Cookie Sandbox:** Agents inherit authenticated state from the human's iksa profile via copy-on-write cookie copies for `share_cookies` domains only (§1.2.4). Cookie values never enter the ledger, audit records, CAS or model requests.
* **Concurrency:** up to `max_agents` (default 3, range 1–8) tasks at once. Tasks the user started (`ask`, `add`, accepting a suggestion, approving or resuming) may use one slot beyond `max_agents`, so they never wait behind scheduled work.

### 3.2 Task Ledger & Proactive Engine
* Task ledger with lifecycle states `Suggested` -> `Queued` -> `Executing` -> `Awaiting Human Hand-off` -> `Committed`, plus terminal `Failed` and `Dismissed`. A hand-off is one of: approval, CAPTCHA challenge, or user pause.
* **Triggers:**
  * **Schedule:** `every <N>m|h|d` (minimum 5 minutes) or 5-field cron in local time; at most 32 schedules. Missed runs collapse to one fire on wake, and a schedule does not fire while its previous task is still open.
  * **Page visit:** a `visit <host[/path]>` schedule fires when a matching URL loads in a human tab (10-minute cooldown, never for `never_send` hosts, HTTP(S) only).
  * **Watch** (a selector's content changes on a URL): not yet planned.
* **Observe first:** every schedule and visit trigger starts in `suggest` mode — its tasks land in the inbox. The user promotes it to `auto` after its acceptance record (shown as `accepted/total`) looks right.
* **Planner:** turns a natural-language goal into validated actions, step by step, using whichever model the user configured. No model is configured by default; proactive features stay off until one is. Each step is one fresh request (cached system prompt + goal + compact step log + current page), at most `max_steps` (default 25). A model that repeats the same action on an unchanged page 3 times fails the task (`stuck`).
* **Page representation:** the model sees a pruned list of interactive and text elements with numbered refs (not raw HTML, never field values). iksa maps each ref to a CSS selector it computes itself, so records, resume and replay work like scripted tasks. Screenshots are sent only if the configured model supports vision and the user enabled it.
* **What leaves the machine:** form field values are never sent; named secrets are typed from the Keychain via `{{secret:NAME}}` placeholders and never reach the model, ledger or snapshots; domains on `never_send` are never sent to a model (the task pauses instead); every model request body is stored in the audit log so the user can see exactly what was shared.
* **Suggestion inbox:** `Suggested` tasks come from `suggest`-mode schedules and visit triggers, and from up to three follow-up goals the planner may propose when a goal commits. They never run until the user accepts; dismissed suggestions are garbage-collected.
* **Attention:** the user gets one digest notification with counts only (`waiting on you: 2 approvals, 1 challenge, 3 suggestions`) — goal text never appears on the lock screen. `iksa list` and the side panel say exactly what each paused task needs. `iksa pause <id>` stops a task before its next step.

### 3.3 Security & Approval Gates
* Configurable risk thresholds: Level 0 (read-only, fully autonomous) to Level 2 (financial transactions, mandatory human sign-off; no setting can auto-approve Level 2). The model may raise a step's risk but never lower it below Write.
* **Standing grants:** `iksa grant <domain> [--schedule <id>] [--days N]` (N ≤ 30, default 7) lets **Write** steps on that domain and its subdomains run without approval, for one schedule or any task. Each use is revalidated against expiry, `never_send` and a fingerprint of the risk settings at grant time. Financial steps never use a grant, and every step a grant allowed records the grant's id.

### 3.4 Content-Addressable Store (CAS) Action Audit Log
* **Deduplication via CAS:** DOM snapshots are split into content-defined chunks, hashed (BLAKE3), stored as immutable zstd blobs. Changesets record only pointers.
* Final tasks compile to human-readable Markdown audit reports, including the goal, model summaries and model-request hashes.
* **Privacy of the log:** data dir is owner-only (0700); typed secrets are redacted and erased from the database and its WAL when a task finishes; Level-2 steps store no DOM snapshot; no snapshot is taken while a secret is on the page; records older than `retention_days` are garbage-collected.

### 3.5 Privacy Baseline
* Chromium launched with background networking, component updates, sync, pings, domain reliability, field trials, crash upload and Safe Browsing pings disabled. Verified by an automated egress test: an idle browser makes zero non-loopback connections, and an idle daemon makes none at all.
* CDP over a pipe, never a TCP port; the profile directory is 0700.
* The only network calls iksa itself makes: `iksa setup` / `iksa update` (download the pinned build), and model calls the user configured.

### 3.6 Extension & Integration Ecosystem
* **MCP Native Host:** Expose the ledger and browser actions as MCP tools; connect to any user-configured model, local or cloud.
* **Extensions:** MV3 extensions run natively in Chromium (no shim); the bundled iksa extension loads with `--load-extension` (CDP `Extensions.loadUnpacked` as fallback). v0.1 targets uBlock Origin and Bitwarden.
* **Research Context System:** Local vectorization (ColBERT/ONNX on blocking worker pools) and hierarchical graph storage, all local.

### 3.7 Configuration
* One file, `~/.iksa/config.json`: `search` (active provider + provider list of `{name, url_template}` with `{q}`), `auto_max` risk level, `max_agents`, `retention_days`, `model` (`provider`, `endpoint`, `name`, `effort`, `max_steps`, `fallbacks`; the API key lives in the OS keychain — never in the file; `vision` on/off in M4), `never_send` (domain list), `share_cookies` (domain list, default empty).
* Schedules, triggers and grants live in the ledger and are managed with `iksa schedule …` and `iksa grant …`, not in the config file.
* **Model providers:** Anthropic (`claude-opus-5-5` documented; Sonnet/Haiku for cost). DeepSeek may be added in M4 only as an explicit opt-in with a warning: its API stores data in the PRC and its terms permit training on submitted data, and its Anthropic-compatible endpoint silently remaps `claude-*` model names, so iksa must refuse those names there.

---

## 4. Technical Challenges & Architecture Open Items

### 4.1 Anti-Bot & CAPTCHA Wall (Human Intervention Hand-off)
* **Challenge:** Agents will hit interactive challenges (Cloudflare Turnstile, hCaptcha, reCAPTCHA).
* **Strategy:** On detection (DOM markers or HTTP 403/503 with Cloudflare fingerprint) execution pauses in `Awaiting Human Hand-off`. The user is notified, opens Agent View, solves the challenge in the agent's own context, clicks "Resume Agent", and the worker continues on the same page. The paused context is held for 15 minutes; after that, resume replays the task's steps from the start.

### 4.2 Human/Agent Race Conditions
* **Challenge:** State corruption if a human and an agent mutate the same site concurrently.
* **Strategy:** Agents work in isolated browser contexts (copy-on-write). A per-domain **mutation lock** holds agent Write and Financial steps while the same domain (or a parent/sub-domain) is open in a human tab: the step waits up to 30 s, then pauses for approval. Read steps are never blocked, and standing grants do not bypass the lock. Cookie diffs sync back only on `Committed`.

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
| **Memory Footprint** | Summed macOS `phys_footprint` over the whole process tree (iksa + Chromium), median of 3 runs: ≤ 300 MB idle; ≤ 1.2 GB for 5 tabs + 3 active background agents. Targets are never raised to fit a feature. |
| **Privacy** | Zero non-loopback connections from an idle browser; no telemetry; no TCP/UDP listeners |
| **Concurrency** | At most `max_agents` (default 3) agent contexts at once, plus one slot for user-started work |
| **Planner quality** | ≥ 16/20 on the M2a eval (15 local fixtures + 5 public sites) with 0 secret or `never_send` leaks |

---

## 6. Milestones

| Milestone | Scope | Status | Plan |
| :--- | :--- | :--- | :--- |
| **M1** Agent core | Pipe CDP client, hidden contexts, scripted tasks, risk gates, CAPTCHA pause, CAS audit, ledger, privacy launch, `setup` | Done 2026-11-03 | `docs/superpowers/plans/2026-10-28-m1-agent-core-v2.md` |
| **M2a** Goal planner | `ask`, observation with refs, Anthropic client, Keychain key and secrets, planner loop, eval | Code done incl. review fixes (`67566e5`, `d414ff5`, `903524c`); live eval not run | `docs/superpowers/plans/2026-11-03-m2a-planner.md`, `docs/superpowers/plans/2026-11-04-m2a-review-fixes.md` |
| **M2b** Proactive | Daemon, schedules, inbox, `max_agents`, follow-up suggestions, grants, pause, stuck detector | Planned | `docs/superpowers/plans/2026-11-04-m2b-proactive.md` |
| **M3** Human browsing | Browsing in iksa's profile, extension side panel, Agent View, shared cookies, visit triggers, mutation lock, address-bar check | Planned | `docs/superpowers/plans/2026-11-04-m3-human-browsing.md` |
| **M4** Remaining v0.1 | MCP host, `iksa update`, OpenAI-compatible and local model providers (DeepSeek opt-in), uBlock Origin/Bitwarden checks, research context system, vision | Planned (plan covers all but the research context system) | `docs/superpowers/plans/2026-11-04-m4-providers-mcp-update.md` |
