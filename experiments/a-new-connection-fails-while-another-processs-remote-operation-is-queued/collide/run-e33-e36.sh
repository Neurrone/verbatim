#!/bin/sh
# Runs each script given N times (N from the environment, default 6), each
# run's output appended to the script's .out file under a "=== run N" line.
exe=/c/Users/Dickson/Desktop/projects/verbatim/target/collide/debug/collide.exe
for script in "$@"; do
  out="${script%.txt}.out"
  : > "$out"
  n=1
  while [ "$n" -le "${N:-6}" ]; do
    echo "=== run $n" >> "$out"
    "$exe" run "$script" >> "$out" 2>&1
    n=$((n + 1))
  done
done
