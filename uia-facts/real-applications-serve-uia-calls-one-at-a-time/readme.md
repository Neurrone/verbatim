# Real applications serve UIA calls one at a time

## Fact

Real applications serve UIA calls as the model predicts (`books/dev/src/explanation/windows/uia.md`, "Where the provider call runs" and "Predicting outcomes"): a suspended UI thread holds every call in Notepad, Edge, VS Code, Firefox, Windows Forms and WPF; Chromium serves page and browser on one thread; one remote operation holds Firefox 2.6 ms and WPF 228 ms; beside Edge's 222 ms `GetText` a cheap call took 220 ms; a `FindAll` with a cache request lets a cheap call in between its requests, the call's median rising only to 0.4 ms, though the call can wait for the whole `Find` request, up to 696 ms beside Edge's 20,000-element page; Windows Terminal's text calls wait while its XAML thread is suspended.

## What Verbatim relies on

D9's per-application outposts (`books/dev/src/explanation/architecture.md`, section 1), which hold that a slow call holds its own application only. That claim is checked against the provider program in [a provider process serves one UIA request at a time](../a-provider-process-serves-one-uia-request-at-a-time/readme.md); this folder holds the applications' records and the search beside a cheap call.

## Method

- `findall`, against the provider program: its window with a tree of 20,000 nodes, a `FindAllBuildCache` of the descendants with the name cached on one thread while another thread reads node 1's name back to back. Judged: some read answers while the search runs (a relation); the reads' median and longest wait and the search's time are printed for trends.

The claim is about how a provider serves calls, and the provider program's cases stand for it here and in [a provider process serves one UIA request at a time](../a-provider-process-serves-one-uia-request-at-a-time/readme.md). No case starts a real application: the per-application figures, the suspended UI thread in Notepad, Edge, VS Code, Firefox, Windows Forms and WPF, Chromium's page and browser on one thread, the operation holds of Firefox and WPF, Edge's `GetText` and Windows Terminal's XAML thread, stay in the documents as their records, untried.

## Client settings

Neither connection recovery nor event coalescing changes these outcomes (checked on 2026-11-06 by running the default run once each by default and with `--recovery`, `--coalescing` and both).

## Recorded results

| Figure | Date | Machine | Source | Threshold |
|---|---|---|---|---|
| A suspended UI thread held every call in Notepad, Edge, VS Code, Firefox, Windows Forms and WPF; Chromium's page and browser on one thread; Firefox 2.6 ms and WPF 228 ms; Edge 222 ms and 220 ms; `FindAll` 0.4 ms median and 696 ms; Windows Terminal 1.24.12741.0's text calls waited | 2026-11-02 | Build 26200 | `experiments/parallel-uia-calls/` (`run-real.ps1`, `wpf-window.ps1`, `winforms-window.ps1`, `wt-serving-thread.ps1`), in git before the commit that moved them here | Relations; times for trends |
| First uia-facts run, `findall`, 3 trials: reads answered during the search 3 of 3; median 0.1 ms, longest 402.1 to 408.9 ms; the search 3,496.2 to 3,564.8 ms | 2026-11-06 | Build 26200.9711, `UIAutomationCore.dll` 7.2.26100.9549, AMD Ryzen 7 5700G | fdfb63ae | Pass; the times for trends |

The provider program's figures are recorded for it afresh, since its tree is not Edge's page.

## How to rerun

`uia-facts/build.ps1`, then `target\debug\real-applications-serve-uia-calls-one-at-a-time.exe`, which runs `findall` and takes over nothing.

## Evidence kept

`recorded/`: the parallel-calls experiment's outputs for the real applications, each named by its application (`edge.txt`, `edge-big-pairs.txt` and so on), with their stacks (`stacks-*.txt`) and the serving-thread traces (`vscode-serving-thread.txt`, `wt-serving-thread.txt` and `wt-text-provider-thread.txt`).
