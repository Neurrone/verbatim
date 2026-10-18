# Experiments: Windows Terminal raises focus from a second runtime identity

These measurements support the application fault recorded in `docs/uia-bugs.md` under [Windows Terminal raises focus from a second runtime identity](../../docs/uia-bugs.md#windows-terminal-raises-focus-from-a-second-runtime-identity). They ran on 2026-10-17 and 2026-10-18 on Dickson's x64 machine (Windows 11, build 26200), against a running Verbatim, through the end-to-end suite; the design record that uses them is `docs/design/windows-terminal-twin-focus.md`, "Rates" and "The judge".

Terms: the usual identity is the terminal control's runtime id that starts 42 with its window's handle; the second identity is the one that starts 14 and carries a counter. A segment is the part of the listener's log from one foreground event of a Windows Terminal window to the next: "launch" is a window's first foreground when it is its process's first window, "second" a second window's first, "return" any later foreground of a known window. An extra announcement is an accepted terminal focus in a segment beyond the first, which Verbatim speaks as a repeated "terminal".

## The judge

The scratch scenario `measure_terminal_identities` opens Windows Terminal and then a second window in the same process and closes both, `VERBATIM_IDENTITY_ROUNDS` times; `identity_judge.py` splits the listener's log into segments and counts, per kind, the segments with any second-identity event, the second identity's focus events received, those accepted, and the extra announcements. `VERBATIM_EXP` selects one of the listener registrations below. The judge was checked on the log of the GitHub run the design record describes, where it counts the one accepted second identity and the two extra announcements.

## Results, 150 rounds per registration

- `judge/baseline`, the listener's registration on main at the time: the second identity was raised in 2 of 150 launches and 19 of 150 second windows, with 4 and 57 focus events from it received; 1 launch accepted it and announced the terminal twice; no second window did.
- `judge/exp-one`, one handler object for the focus handler and every part of the group, as NVDA registers its one handler object: 2 of 150 launches and 11 of 150 second windows; 2 launches announced it twice.
- `judge/exp-full`, NVDA's whole global group on the root with the listener's own handlers: none of 150 launches and 19 of 150 second windows; none announced it twice.
- No return segment had a second-identity event in any run.

`suffix.py` checked what stays equal between the two identities: in all 156 focus events from a second identity in these runs and the GitHub run, a focus from the usual identity in the same process arrived within 1 second, and the usual identity's runtime id after its `[42, window]` prefix equalled the last two parts of the second identity's (design record, "What stays equal between the two identities").

Established, on this machine: the second identity occurs with every registration tried, in about 1 in 8 to 1 in 14 second windows; at launch the counts are too few to tell the registrations apart. NVDA receives it in second windows too (6 times in 15 pairs, `docs/design/focus-pipeline.md`, section 16.8).

## Files

- `identity_judge.py`: the judge (`python -I identity_judge.py <listener.log or folder of runs> [--verbose]`).
- `suffix.py`: the runtime-id comparison.
- `identity-judge-and-experiments.patch`: the scratch scenario and the two registration experiments, against 32948a4d.
- `judge/baseline/`, `judge/exp-one/`, `judge/exp-full/`: each run's judge output (`judge.txt`) and test result (`test.out`). The listener logs the judge read, about 5 MB each, and the timelines are not kept. The scratch folder in the judge's paths is redacted to `<scratch folder>`.

The GitHub runs the design record cites (Actions runs [38988118089](https://github.com/Neurrone/verbatim/actions/runs/38988118089) and [39032739474](https://github.com/Neurrone/verbatim/actions/runs/39032739474)) are not copied here: their artifacts, about 200 MB in all, expire on GitHub, and the findings this entry needs are recorded above.

## Rerunning

Apply the patch to a scratch worktree of 32948a4d, build, start `verbatim-agent` as `docs/tooling/e2e.md` says, and run `cargo test -p verbatim-e2e --test e2e -- --ignored --exact measure_terminal_identities --test-threads=1` with `VERBATIM_IDENTITY_ROUNDS=150` and, for an experiment, `VERBATIM_EXP` set to `one` or `full`; then run the judge on the kept listener log. It takes over the desktop for about 11 minutes per 150 rounds.
