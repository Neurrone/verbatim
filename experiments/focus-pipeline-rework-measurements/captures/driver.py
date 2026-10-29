"""Holds mockapp's stdin for the by-hand captures of the focus pipeline
rework's step 1 (docs/design/focus-pipeline-rework.md, "Order of work").

Usage: python driver.py <log> <command folder> -- <mockapp command line>

Starts mockapp, waits for its "ready", then runs each "*.cmd" file that
appears in the command folder, in name order, one line at a time:
"sleep <ms>" waits, "touch <path>" writes an empty file (for a capture's
--wait-file step), "quit" ends mockapp and the driver, and any other line
is sent to mockapp, waiting for its acknowledgement ("applied",
"rejected: ...", or for a stall "stall started"). When a file is done,
"<name>.done" is written beside it. Every line sent and received goes to
the log with the wall-clock time in microseconds since the Unix epoch,
the clock mockapp's own stamps use.
"""

import pathlib
import queue
import subprocess
import sys
import threading
import time


def now_us():
    return time.time_ns() // 1000


def main():
    log_path = pathlib.Path(sys.argv[1])
    folder = pathlib.Path(sys.argv[2])
    command = sys.argv[sys.argv.index("--") + 1 :]
    folder.mkdir(parents=True, exist_ok=True)
    log = log_path.open("a", encoding="utf-8", buffering=1)

    def write(kind, text):
        log.write(f"{now_us()} {kind} {text}\n")

    write("start", " ".join(command))
    child = subprocess.Popen(
        command,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        bufsize=1,
    )
    acks = queue.Queue()
    ready = threading.Event()

    def read():
        for line in child.stdout:
            line = line.rstrip("\r\n")
            write("out", line)
            if line == "ready":
                ready.set()
            elif line == "applied" or line.startswith("rejected") or line == "stall started":
                acks.put(line)
        write("exit", str(child.wait()))

    threading.Thread(target=read, daemon=True).start()
    if not ready.wait(30):
        write("error", "no ready within 30 s")
        child.kill()
        return
    while True:
        for path in sorted(folder.glob("*.cmd")):
            for line in path.read_text(encoding="utf-8").splitlines():
                line = line.strip()
                if not line:
                    continue
                if line.startswith("sleep "):
                    write("sleep", line[6:])
                    time.sleep(int(line[6:]) / 1000)
                    continue
                if line.startswith("touch "):
                    pathlib.Path(line[6:]).write_text("")
                    write("touch", line[6:])
                    continue
                # Acknowledgements left over from earlier commands, such as
                # the command a stall applies part way, or an on-activate
                # command when it runs, are dropped first.
                while not acks.empty():
                    acks.get_nowait()
                write("in", line)
                child.stdin.write(line + "\n")
                child.stdin.flush()
                if line == "quit":
                    child.wait(10)
                    path.rename(path.with_suffix(".done"))
                    return
                stall = line.startswith("stall ")
                deadline = time.monotonic() + 40
                while True:
                    try:
                        ack = acks.get(timeout=max(0.0, deadline - time.monotonic()))
                    except queue.Empty:
                        write("error", f"no acknowledgement for {line!r}")
                        break
                    if (ack == "stall started") == stall:
                        break
            path.rename(path.with_suffix(".done"))
        if child.poll() is not None:
            return
        time.sleep(0.05)


main()
