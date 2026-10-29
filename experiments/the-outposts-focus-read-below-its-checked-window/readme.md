# Experiment: the outpost's focus read below its checked window

This experiment serves `docs/design/outpost-focused-element-study.md`. It asks whether the outpost could find its application's keyboard focus from the focus window it has checked as its application's, by a connection by handle and a search below that window, in place of `GetFocusedElementBuildCache`, and what the search costs. It ran on Dickson's x64 machine (Windows 11, build 26200) on 2026-10-29, in a debug build at 984a3a74, against mockapp's UIA backend, each case on a desktop of its own (`harness::run_isolated`), with no other client on that desktop and seven cases running in parallel.

## Setup

`focus_by_handle_study.rs` is the scratch test binary that ran, kept here and not built. `scratch-support.patch` adds its `[[test]]` entry to mockapp's `Cargo.toml`. `make_fixtures.py` writes the four generated fixtures it uses into `crates/mockapp/tests/fixtures/`: flat lists of 150, 250 and 400 items (`study_wide150.json`, `study_wide250.json`, `study_wide400.json`) and a list of 120 items with four cells each (`study_grid120.json`), the shape of a file list's details view. The other fixtures are the repository's: `tree.json`, `deep.json` (a list 60 groups deep) and `wide.json` (a list of 100 items).

Each case starts mockapp, moves its focus with `focus <id>` to the item named below (the last item of each list, the deepest item of `deep.json`, and `tree.json`'s first list item, whose parent has no window of its own), and then:

- reads the window's element by handle with every node detail cached (`Uia::window_element`), and its cached `HasKeyboardFocus`;
- runs a remote program that walks the window's raw-view descendants depth first until one matches, in three forms: `HasKeyboardFocus` true (`focus`); the same, not descending into an element that is a window of its own (`focus-pruned`); and the focused node's runtime id (`runtime-id`). Each form runs once in its counting form, for the executed instructions (`verbatim_uia_rops::counting`, valid only under half the limit), and once plainly, timed, with mockapp's provider calls counted (`common::read_hits`);
- runs one `FindFirstBuildCache` for `HasKeyboardFocus` true over the window's descendants (`findfirst`), timed and with its provider calls counted.

Each search's answer is compared with the focused node found by its name. On a test's own desktop nothing has the system focus, so `GetFocusedElement` answers the desktop and cannot be aimed at mockapp (`docs/design/focus-pipeline.md`, section 11); mockapp's provider answers `GetFocus` and `HasKeyboardFocus` from the same state, so agreement here holds by construction and says nothing of real providers.

The `tree.json` case also times, 1,000 times, the local work `focused_control_in` (`crates/verbatim-outpost/src/outpost/read.rs`) does between reading the focus window and calling `GetFocusedElementBuildCache`, with a kept verdict and a client already made: `GetGUIThreadInfo`, the provider-process check, `IsWindow`, `IsHungAppWindow`, `GetClassNameW`, and building the node and ancestor cache requests.

Rerunning: at 984a3a74 or later, apply the patch, copy the test file to `crates/mockapp/tests/`, run `make_fixtures.py` in `crates/mockapp/tests/fixtures/`, and run `cargo test -p mockapp --test focus_by_handle_study`. Each case prints `RESULT` lines; `results.txt` is the run reported here.

## Results

Elements looked at count six elements of the window's non-client area, which UIA's own proxy serves in the raw view before the fixture's first element.

- Instructions per element looked at: about 21 for the `HasKeyboardFocus` search (348 at 16 elements, 1,095 at 69, 2,253 at 107, 3,303 at 157), about 28 with the pruning (454, 1,572, 2,996, 4,396), and about 26 for the runtime-id search (427, 1,439, 2,787, 4,087).
- The limit of 10,000 executed instructions: the `HasKeyboardFocus` search answered at 257 and 407 elements and was stopped by the limit in the grid, whose focus comes after about 600; the pruned search answered at 257 and was stopped at 407; the runtime-id search answered at 257 and was stopped at 407. So one run reaches about 470, 355 and 385 elements respectively, and about half that under the margin `docs/performance.md` asks of a program's worst case.
- Every search that answered found the focused node.
- Provider calls: about 16 per element looked at for the remote search and as many for `FindFirstBuildCache` (1,764 and 1,595 in `wide.json`; 240 and 236 in `tree.json`), most of them `ProviderOptions` that UIA asks for each element it handles. `FindFirstBuildCache` found the focus in every case, the grid's included, at 9,868 provider calls there. The connection by handle with every node detail cached cost 48.
- Wall-clock times were 17 to 31 ms a search, about 20 ms even for 16 elements, with seven cases in parallel on one machine; they are not a cost figure.
- The local work of the span between the check and the call: 65 and 69 microseconds at the median in two runs, 122 and 419 at the 99th percentile, 201 and 763 at worst.
