# Spike reproduction kit
Run from the repository root with the pinned Chromium already in `.spike-home/chromium/`.
`python3 spikes/raw_probe.py s3` reproduces the egress probe.
`python3 spikes/raw_probe.py s5` reproduces the memory probe.
Runtime profiles, logs, and evidence stay under `.spike-home/`; run browsers serially.
The whole `spikes/` directory is deleted at the end of Task 8.
