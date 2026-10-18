#!/bin/sh
# Runs e58's trial N times with E58_CASE (and any other E58_* setting) in
# the environment and appends each trial's summary line to out/<label>.out.
# usage: E58_CASE=silent ./run.sh N label
# EXE is the uia_bugs test binary built with e58's hooks (see readme.md).
EXE=${EXE:?set EXE to the uia_bugs test binary}
n=$1; label=$2
mkdir -p out
for i in $(seq 1 "$n"); do
  output=$("$EXE" --exact e58_trial 2>&1)
  line=$(echo "$output" | grep '^e58 ')
  if [ -z "$line" ]; then
    echo "FAIL: $(echo "$output" | tail -5 | tr '\n' ' ')" | tee -a "out/$label.out"
  else
    echo "$line" | tee -a "out/$label.out"
  fi
done
