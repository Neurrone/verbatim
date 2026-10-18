"""Runs e37: each trial is one `collide run` of a script with a fresh
console and fresh client processes, the delay after the console window
exists given as DELAY. Each set's output goes to <script>.out, one
"=== run N delay=D" line before each trial. The sets are interleaved
trial by trial so that any drift over the run touches each alike.

Delays per set: 12 at 0 ms (as early as the window can be found), 48
drawn uniformly from 0 to 100 ms (seeded), for the three early sets; and
5 at 2000 ms for each control set.
"""
import os
import random
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).parent
EXE = r"C:\Users\Dickson\Desktop\projects\verbatim\target\collide\debug\collide.exe"
rng = random.Random(37)


def delays():
    out = [0.0] * 12 + [round(rng.uniform(0, 100), 1) for _ in range(48)]
    rng.shuffle(out)
    return out


SETS = {
    "e37-parallel": delays(),
    "e37-rawfirst": delays(),
    "e37-alone": delays(),
}
CONTROLS = {
    "e37-parallel-control": ("e37-parallel", [2000.0] * 5),
    "e37-rawfirst-control": ("e37-rawfirst", [2000.0] * 5),
}

outs = {}
plan = []
for name, ds in SETS.items():
    outs[name] = open(HERE / f"{name}.out", "w", encoding="utf-8")
for name in CONTROLS:
    outs[name] = open(HERE / f"{name}.out", "w", encoding="utf-8")
n = max(len(d) for d in SETS.values())
for i in range(n):
    for name, ds in SETS.items():
        if i < len(ds):
            plan.append((name, name, i + 1, ds[i]))
for name, (script, ds) in CONTROLS.items():
    for i, d in enumerate(ds):
        plan.append((name, script, i + 1, d))

for name, script, run, delay in plan:
    title = f"e37-{name[4:]}-{run}-{os.getpid()}"
    env = dict(os.environ, TITLE=title, DELAY=str(delay))
    out = outs[name]
    out.write(f"=== run {run} delay={delay}\n")
    out.flush()
    result = subprocess.run([EXE, "run", str(HERE / f"{script}.txt")], env=env,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                            encoding="utf-8", errors="replace", timeout=60)
    out.write(result.stdout)
    out.flush()
    time.sleep(0.5)  # lets the killed console's window go before the next
for f in outs.values():
    f.close()
print("done", len(plan), "trials", file=sys.stderr)
