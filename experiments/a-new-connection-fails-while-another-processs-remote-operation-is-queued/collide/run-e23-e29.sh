#!/bin/sh
# Runs each script given three times, each run's output appended to the
# script's .out file under a "=== run N" line.
exe=/c/Users/Dickson/Desktop/projects/verbatim/target/lock-exp/debug/collide.exe
for script in "$@"; do
  out="${script%.txt}.out"
  : > "$out"
  for n in 1 2 3; do
    echo "=== run $n" >> "$out"
    "$exe" run "$script" >> "$out" 2>&1
  done
done
