# Brief 1 — result

Task 1 only; no production scaffold, Task 2+, or document rename.

| Spike | Result | Evidence / decision |
| --- | --- | --- |
| S1 Chromium | PASS | SHA-256 verified; headful CDP returns Chrome/156.0.8078.47. |
| S2 Hidden targets | PARTIAL | Raw CDP works; Rust Page subcheck BLOCKED: `Requested value not found.` / `deadline has elapsed`; hidden frames 0, visible 3. |
| S3 Idle egress | PASS | After 60 s, only 127.0.0.1 CDP sockets; no non-loopback endpoint observed. |
| S4 Extension / native host | PARTIAL | Effective DDG default and native ping-pong verified; fresh-profile bubble observation BLOCKED: `No on-screen scoped Chromium windows available`. |
| S5 Memory | FAIL | Idle 1,015,952 KiB; hidden Wikipedia 1,322,544 KiB; idle exceeds ~300 MB. |
| S6 Bot visibility | PASS | Both visible and hidden show Turnstile markers; no interstitial; observation only. |

## Plan changes

- Stop before Task 2+: the idle memory target fails and hidden Page integration is unresolved. Options for review: revise the browser/pin/target budget, or investigate an explicitly scoped follow-up spike; do not silently relax the target.
- Task 8 must translate shell-form `--flags` into chromiumoxide argument keys without `--`, and explicitly support hidden targets whose CDP type is `other` (adapter/upstream patch/dependency decision). A visible fallback changes the no-window requirement and needs a decision.
- M3 must use `.extension(path)`; `.disable_default_args()` alone still emits `--disable-extensions` in chromiumoxide 0.9.1.
- M3 cannot rely on hidden-target screencasts on this pin. A new visible target in the same context preserves cookies, but loses DOM/heap/form/challenge-local state. `activateTarget` does not reveal the existing hidden page.
- macOS throwaway runs require `--use-mock-keychain` to avoid the user-observed missing-keychain modal. Its fixed key is appropriate for these disposable profiles; production keychain handling needs its own decision.
- Register native hosts under `<user-data-dir>/NativeMessagingHosts/`; keep the profile 0700. No real-home host installation was performed.

Evidence and exact commands: `docs/spikes.md`; runtime logs: `.spike-home/logs/`.
