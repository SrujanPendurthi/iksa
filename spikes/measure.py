"""Sample RSS and macOS phys_footprint over one experiment process tree."""
import os
import pathlib
import re
import subprocess
import sys

rows = subprocess.check_output(['ps', '-axo', 'pid=,ppid=,rss=,command='], text=True).splitlines()
procs = {}
for row in rows:
    p, pp, rss, cmd = row.strip().split(None, 3)
    procs[int(p)] = (int(pp), int(rss), cmd)
seeds = {int(sys.argv[2]) if len(sys.argv) > 2 else os.getppid()}
while True:
    extra = {p for p, (pp, _, _) in procs.items() if pp in seeds}
    if extra <= seeds:
        break
    seeds |= extra

def helper(cmd):
    name = pathlib.Path(cmd.split()[0]).name.lower()
    return name.startswith(('python', 'ps', 'footprint'))

pids = sorted(p for p in seeds if p in procs and not helper(procs[p][2]))
label = sys.argv[1]
print(label, 'pids=', pids, 'RSS_KiB=' + str(sum(procs[p][1] for p in pids)), flush=True)
values = []
errors = []
for pid in pids:
    print('PROCESS', pid, procs[pid], flush=True)
    try:
        result = subprocess.run(['footprint', '-p', str(pid), '-f', 'bytes'], capture_output=True, text=True, timeout=30)
        print('FOOTPRINT_RAW_BEGIN', pid, 'exit=' + str(result.returncode), flush=True)
        print(result.stdout, end='', flush=True)
        print(result.stderr, end='', flush=True)
        print('FOOTPRINT_RAW_END', pid, flush=True)
        match = re.search(r'^\s*phys_footprint:\s*([\d.]+)\s*(B|KB|MB|GB)?\s*$', result.stdout, re.M)
        if result.returncode or not match:
            raise RuntimeError('footprint exit=' + str(result.returncode) + '; ' + (result.stderr.strip() or 'phys_footprint line missing'))
        # Request exact bytes, then normalize to KiB (older formatted units also accepted).
        value = float(match[1]) * {'B': 1 / 1024, 'KB': 1, 'MB': 1024, 'GB': 1024**2}[match[2] or 'B']
        values.append(value)
        print('PID_FOOTPRINT_KiB=' + str(value), 'pid=' + str(pid), flush=True)
    except (RuntimeError, subprocess.TimeoutExpired) as error:
        errors.append(str(error))
        print('BLOCKED', 'pid=' + str(pid), repr(error), flush=True)
if errors or not pids:
    print(label, 'FOOTPRINT_KiB=BLOCKED', repr(errors or ['empty PID set']), flush=True)
else:
    print(label, 'FOOTPRINT_KiB=' + str(sum(values)), flush=True)
print(subprocess.run(['lsof', '-nP', '-a', '-p', ','.join(map(str, pids)), '-i'], capture_output=True, text=True).stdout)
