# Shell helpers for the by-hand captures (sourced, never run on its own).
C=/c/Users/Dickson/Desktop/projects/verbatim/experiments/focus-pipeline-rework-measurements/captures
R=/c/Users/Dickson/Desktop/projects/verbatim
W=$(cygpath -w "$C")
# cmd <folder> <name> <lines...>: queues a command file for the driver.
cmd() { local d=$1 n=$2; shift 2; printf '%s\n' "$@" > "$d/$n.tmp" && mv "$d/$n.tmp" "$d/$n.cmd"; }
# done_wait <folder> <name>: waits up to 60 s for the driver to finish it.
done_wait() { for i in $(seq 600); do [ -f "$1/$2.done" ] && return 0; sleep 0.1; done; echo "timed out on $2"; return 1; }
# cap <args...>: runs a capture through the agent, printing wall time first.
cap() { echo "capture started at $(date +%s%6N) us: $*"; (cd $R && target/debug/xtask.exe nvda capture "$@" 2>&1); }
