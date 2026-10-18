"""Runs e39 sets as e38 did: each trial is one `collide run` of a script
with fresh consoles and fresh processes, the delay after the console
window is found given as DELAY, the sets interleaved trial by trial, each
set's output appended to <set>.out after an "=== run N delay=D" line.
After each trial, any hook log the trial's processes wrote is moved to
hooklogs/<set>-<run>-<pid>.log. Usage: run-e39.py <trials per set>
<delay ms> <set>...
"""
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).parent
EXE = r"C:\Users\Dickson\Desktop\projects\verbatim\target\collide\debug\collide.exe"
HOOKLOG = Path(r"C:\Users\Dickson\Desktop\projects\verbatim\target\collide\hooklog")
n = int(sys.argv[1])
delay = float(sys.argv[2])
sets = sys.argv[3:]
(HERE / "hooklogs").mkdir(exist_ok=True)
start = {name: sum(1 for l in open(HERE / f"{name}.out", encoding="utf-8") if l.startswith("=== run ")) if (HERE / f"{name}.out").exists() else 0 for name in sets}
for i in range(n):
    for name in sets:
        run = start[name] + i + 1
        title = f"e39-{name[4:]}-{run}-{os.getpid()}"
        env = dict(os.environ, TITLE=title, DELAY=str(delay))
        with open(HERE / f"{name}.out", "a", encoding="utf-8") as out:
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
        if HOOKLOG.exists():
            for f in HOOKLOG.glob("*.log"):
                try:
                    shutil.move(str(f), HERE / "hooklogs" / f"{name}-{run}-{f.name}")
                except OSError:
                    pass
        time.sleep(0.5)
print("done", n * len(sets), "trials", file=sys.stderr)
