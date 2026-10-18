#!/bin/sh
# Runs e57's trial N times with the E57_* settings in the environment and
# appends each trial's summary line to out/<label>.out.
# usage: E57_CTO=2000 E57_RAISES=0,50,100 ./run.sh N label
# EXE is the uia_bugs test binary built with e57's hooks (see readme.md).
EXE=${EXE:?set EXE to the uia_bugs test binary}
n=$1; label=$2
mkdir -p out
for i in $(seq 1 "$n"); do
  output=$("$EXE" --exact e57_trial 2>&1)
  line=$(echo "$output" | grep '^e57 ')
  if [ -z "$line" ]; then
    echo "FAIL: $(echo "$output" | tail -5 | tr '\n' ' ')" | tee -a "out/$label.out"
  else
    echo "$line" | tee -a "out/$label.out"
  fi
done
