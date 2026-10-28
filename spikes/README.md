# Task 1 throwaway probes

Run from repository root. Browser download/install and findings are recorded in
`../docs/spikes.md`. These are feasibility experiments, not the iksa application.
The Rust binaries require the pinned app at
`.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium`.

```sh
python3 spikes/prepare.py
TMPDIR="$PWD/.spike-home/tmp" CARGO_TARGET_DIR="$PWD/spikes/target" cargo build --manifest-path spikes/Cargo.toml
python3 -m http.server 8765 --bind 127.0.0.1 --directory .spike-home/www
```

The common Rust builder itself enforces child HOME, CFFIXED_USER_HOME and TMPDIR under `.spike-home` via `.env()`; raw Python launches do the same. The following explicit exports also scope auxiliary shell commands. `spikes/target/debug/s1 --verify-env` verifies the child environment map and runs the CDP version check.

In a second shell, preserve the real Cargo/Rust paths while isolating browser
runtime home, temporary files, profiles, downloads and evidence:

```sh
export HOME="$PWD/.spike-home"
export CFFIXED_USER_HOME="$HOME"
export TMPDIR="$HOME/tmp"
spikes/target/debug/s1
spikes/target/debug/experiments s2
spikes/target/debug/experiments s3
SPIKE_EXTENSION="$PWD/spikes/ext" spikes/target/debug/s4
python3 spikes/raw_probe.py s2
python3 spikes/raw_probe.py s5
python3 spikes/raw_probe.py s6
```

Run browsers serially. All launch paths include `--use-mock-keychain` for these
throwaway profiles. It uses a fixed encryption key and is not a production policy.
Rust S2 records chromiumoxide's unsupported hidden target (`type: other`);
raw CDP distinguishes protocol capability from that crate limitation.
`measure.py` includes the driver and descendants (including measurement helper)
in RSS. macOS summed RSS is conservative shared-page accounting.
Native path A/B experiments must enable only one manifest location per run;
never create a manifest in the real home directory.

S4 settings checks have a 15s deadline and save rendered effective-default text/screenshots. Optional `SPIKE_PROFILE_NAME` is restricted to a single safe name under `.spike-home`; prepare its native host manifest there before running a fresh-profile native check. `capture_window.py` captures only a window owned by the exact scoped Chromium executable PID, and fails explicitly when no on-screen window exists. S5 always creates a unique profile and records idle/loaded target inventories.
