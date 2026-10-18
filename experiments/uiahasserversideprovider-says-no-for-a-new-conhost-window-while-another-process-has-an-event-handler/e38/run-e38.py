"""Runs e38: each trial is one `collide run` of a script with fresh
consoles and fresh processes, the delay after the (second) console window
is found given as DELAY. Each set's output goes to <script>.out, one
"=== run N delay=D" line before each trial. The sets are interleaved trial
by trial. Usage: run-e38.py <trials per set> <seed> <set>...; the delays
are a fifth at 0 ms and the rest drawn uniformly from 0 to 100 ms, or all
0 ms when the seed is "zero".
Outputs are appended to, so a second invocation adds trials.
"""
import os
import random
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).parent
EXE = r"C:\Users\Dickson\Desktop\projects\verbatim\target\collide\debug\collide.exe"
n = int(sys.argv[1])
zero = sys.argv[2] == "zero"
rng = random.Random(0 if zero else int(sys.argv[2]))
sets = sys.argv[3:]


def delays():
    if zero:
        return [0.0] * n
    out = [0.0] * (n // 5) + [round(rng.uniform(0, 100), 1) for _ in range(n - n // 5)]
    rng.shuffle(out)
    return out


plan = {name: delays() for name in sets}
outs = {name: open(HERE / f"{name}.out", "a", encoding="utf-8") for name in sets}
start = {name: sum(1 for l in open(HERE / f"{name}.out", encoding="utf-8") if l.startswith("=== run ")) if (HERE / f"{name}.out").exists() else 0 for name in sets}
for i in range(n):
    for name in sets:
        run = start[name] + i + 1
        delay = plan[name][i]
        title = f"e38-{name[4:]}-{run}-{os.getpid()}"
        env = dict(os.environ, TITLE=title, DELAY=str(delay))
        out = outs[name]
        out.write(f"=== run {run} delay={delay}\n")
        out.flush()
        try:
            result = subprocess.run([EXE, "run", str(HERE / f"{name}.txt")], env=env,
                                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
                                    encoding="utf-8", errors="replace", timeout=90)
            out.write(result.stdout)
        except subprocess.TimeoutExpired as e:
            out.write((e.stdout or b"").decode("utf-8", "replace") if isinstance(e.stdout, bytes) else (e.stdout or ""))
            out.write("=== trial timed out\n")
        out.flush()
        time.sleep(0.5)
for f in outs.values():
    f.close()
print("done", n * len(sets), "trials", file=sys.stderr)
