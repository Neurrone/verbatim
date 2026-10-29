"""Tallies the focus pipeline rework's step 1 measurement runs.

Scratch tooling, never merged (docs/design/focus-pipeline-rework.md, "Order
of work", step 1). Reads run folders, each the artifacts of one end-to-end
run of the scratch measurement build as batch.ps1 copies them, and prints
the inputs to the step's rules ("The rules the results feed"), per scenario
and in total.

Usage: python -I tally.py <folder> [<folder> ...]

Every folder given is searched for run folders: a folder holding
listener.log or summary.txt. A run's scenario is the name in its
summary.txt, else the name of the folder above it.

The lines read, all under the log target rework_measure but the gap's:

- listener.log: what="uia_focus" (one per UIA focus the listener judged:
  NVDA's duplicate test against the reading thread's kept read, the kept
  read's currency, and today's outcome) and what="uia_menu" (one per UIA
  menu opening, and whether today's listener dropped it behind a waiting
  focus).
- outpost-*.log: what="msaa_focus" (one per MSAA focus that reached the
  duplicate test), what="hook_in" and what="forwarding_ended"; and the
  outpost's what="focused_read" (one per GetFocusedElementBuildCache the
  outpost made, for a focus-now answer or a gap read); and the
  outpost's own lines for the gap: "MSAA focus dropped: UIA owns the
  window" (a WinEvent copy in a UIA window) and "no UIA focus followed a
  WinEvent focus in a UIA window" (a firing), with its uia_focus_before
  and sender_gone fields.
- stderr.log (verbatim.exe): what="core_focus" (one per focus report Core
  reduced), what="text_sent" and what="text_reduced".

Definitions:

- A UIA repeat by NVDA's test: nvda_repeat=true. An MSAA repeat by NVDA's
  test: at_kept=true and kept_not_simple=true.
- Current: the line's current=true, the Terms' rule as the patch computes
  it.
- Core did not hold it: the node of the kept read's report (the core_focus
  line whose trace is the line's kept_trace) is not Core's focus as of the
  repeat's judgement (focus_after of the last core_focus line applied
  before the line's judged_us). A kept read whose report never reached
  Core counts as not held, and is counted apart too.
- A menu opening with a focus of its application observed before and
  applied after: a core_focus line of the menu's pid, not the menu's own,
  observed before the menu's observation and applied after the menu's own
  report was applied, or after the menu's observation when the menu never
  reached Core (dropped today behind a focus, or by its read).
- A refusal while Core held another node of that application: a
  core_focus line with taken=false and held_other_of_app=true.
- The hold's cost: in one outpost log, the time from hook_in to the next
  forwarding_ended.
- A mismatch: a focused_read line with differ=true, the element the call
  found being in another process than the checked window's provider. A
  read that failed (error not empty) has no element to compare and is
  counted apart.
- A text request's round trip: text_reduced's at_us less text_sent's, by
  query id within one run; caret watches (op AwaitCaret), which wait for
  the application to act, are left out.

Percentiles are the sample at the rounded index p/100 times (n-1) of the
sorted samples, as docs/tooling/measuring.md defines them.
"""

import os
import re
import sys
from collections import Counter, defaultdict

ANSI = re.compile(r"\x1b\[[0-9;]*m")
FIELD = re.compile(r'(\w+)=("(?:[^"\\]|\\.)*"|\S+)')
GAP_COPY = "MSAA focus dropped: UIA owns the window"
GAP_FIRING = "no UIA focus followed a WinEvent focus in a UIA window"


def fields(line):
    found = {}
    for key, value in FIELD.findall(line):
        if value.startswith('"') and value.endswith('"'):
            value = value[1:-1]
        found[key] = value
    return found


def truth(value):
    return value == "true"


def number(value):
    try:
        return int(value)
    except (TypeError, ValueError):
        return 0


def percentile(samples, p):
    ordered = sorted(samples)
    return ordered[(p * (len(ordered) - 1) + 50) // 100]


def figures(samples, unit_divisor=1000.0, unit="ms"):
    if not samples:
        return "none"
    return (
        f"{len(samples)} samples, median {percentile(samples, 50) / unit_divisor:.1f} {unit}, "
        f"90th percentile {percentile(samples, 90) / unit_divisor:.1f} {unit}, "
        f"max {max(samples) / unit_divisor:.1f} {unit}"
    )


def lines_of(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as handle:
            for line in handle:
                yield ANSI.sub("", line.rstrip("\n"))
    except OSError:
        return


def measure_lines(path):
    for line in lines_of(path):
        if "rework_measure" in line and " measure " in line:
            yield fields(line)


def scenario_of(folder):
    for line in lines_of(os.path.join(folder, "summary.txt")):
        if line.startswith("name: "):
            return line[len("name: "):].strip()
    return os.path.basename(os.path.dirname(os.path.abspath(folder)))


def run_folders(roots):
    for root in roots:
        for folder, _, files in os.walk(root):
            if "listener.log" in files or "summary.txt" in files:
                yield folder


class Tally:
    def __init__(self):
        self.runs = 0
        self.passed = 0
        self.focus = {"uia": Counter(), "msaa": Counter()}
        self.menus = Counter()
        self.refusals = Counter()
        self.refusal_verdicts = Counter()
        self.outpost_starts = 0
        self.unended_starts = 0
        self.hold_us = []
        self.gap_copies = 0
        self.gap_copies_opened = 0
        self.gap_firings = Counter()
        self.text_us = defaultdict(list)
        self.focused_reads = Counter()

    def add(self, other):
        self.runs += other.runs
        self.passed += other.passed
        for kind in self.focus:
            self.focus[kind].update(other.focus[kind])
        self.menus.update(other.menus)
        self.refusals.update(other.refusals)
        self.refusal_verdicts.update(other.refusal_verdicts)
        self.outpost_starts += other.outpost_starts
        self.unended_starts += other.unended_starts
        self.hold_us.extend(other.hold_us)
        self.gap_copies += other.gap_copies
        self.gap_copies_opened += other.gap_copies_opened
        self.gap_firings.update(other.gap_firings)
        for op, samples in other.text_us.items():
            self.text_us[op].extend(samples)
        self.focused_reads.update(other.focused_reads)


def tally_run(folder):
    tally = Tally()
    tally.runs = 1
    result = " ".join(lines_of(os.path.join(folder, "result.txt")))
    summary = " ".join(lines_of(os.path.join(folder, "summary.txt")))
    if result.startswith("pass") or (not result and "result: pass" in summary):
        tally.passed = 1

    core = [line for line in measure_lines(os.path.join(folder, "stderr.log")) if line.get("what") == "core_focus"]
    core.sort(key=lambda line: number(line.get("applied_us")))
    node_of_trace = {}
    applied_of_trace = {}
    for line in core:
        node_of_trace.setdefault(line.get("trace"), line.get("node"))
        applied_of_trace.setdefault(line.get("trace"), number(line.get("applied_us")))

    def core_focus_at(at_us):
        focus = ""
        for line in core:
            if number(line.get("applied_us")) > at_us:
                break
            focus = line.get("focus_after", "")
        return focus

    def judge_repeat(kind, line, repeat):
        counts = tally.focus[kind]
        counts["focuses"] += 1
        if not repeat:
            return
        counts["repeats"] += 1
        current = truth(line.get("current"))
        if current:
            counts["current"] += 1
        kept_node = node_of_trace.get(line.get("kept_trace"))
        if kept_node is None:
            counts["kept read never reached Core"] += 1
        held = kept_node is not None and core_focus_at(number(line.get("judged_us"))) == kept_node
        if not held:
            counts["not held"] += 1
            if current:
                counts["current and not held"] += 1

    listener = list(measure_lines(os.path.join(folder, "listener.log")))
    for line in listener:
        if line.get("what") == "uia_focus":
            judge_repeat("uia", line, truth(line.get("nvda_repeat")))
            if line.get("outcome") == "a repeat of the last focus, which still has the keyboard focus":
                tally.focus["uia"]["dropped as a repeat today"] += 1
    for line in listener:
        if line.get("what") != "uia_menu":
            continue
        tally.menus["openings"] += 1
        pid = line.get("pid")
        observed = number(line.get("observed_us"))
        trace = line.get("trace")
        dropped = truth(line.get("dropped_behind_focus"))
        if dropped:
            tally.menus["dropped behind a focus today"] += 1
        reference = applied_of_trace.get(trace) if trace else None
        if reference is None:
            tally.menus["never reached Core"] += 1
            reference = observed
        pending = any(
            focus.get("pid") == pid
            and focus.get("trace") != trace
            and 0 < number(focus.get("observed_us")) < observed
            and number(focus.get("applied_us")) > reference
            for focus in core
        )
        if pending:
            tally.menus["with a focus observed before and applied after"] += 1

    for line in core:
        tally.refusals["focus reports"] += 1
        if not truth(line.get("taken")):
            tally.refusals["not taken"] += 1
            if truth(line.get("held_other_of_app")):
                tally.refusals["while Core held another node of that application"] += 1
                tally.refusal_verdicts[line.get("verdict") or "(no verdict)"] += 1

    for name in sorted(os.listdir(folder)):
        if not (name.startswith("outpost-") and name.endswith(".log")):
            continue
        hook_in = None
        for line in lines_of(os.path.join(folder, name)):
            if GAP_COPY in line:
                tally.gap_copies += 1
                if "opened_gap=true" in line:
                    tally.gap_copies_opened += 1
                continue
            if GAP_FIRING in line:
                found = fields(line)
                tally.gap_firings[
                    (found.get("uia_focus_before", "(absent)"), found.get("sender_gone", "(absent)"))
                ] += 1
                continue
            if "rework_measure" not in line or " measure " not in line:
                continue
            found = fields(line)
            what = found.get("what")
            if what == "msaa_focus":
                judge_repeat(
                    "msaa", found, truth(found.get("at_kept")) and truth(found.get("kept_not_simple"))
                )
                if truth(found.get("today_repeat")):
                    tally.focus["msaa"]["dropped as a repeat today"] += 1
            elif what == "focused_read":
                tally.focused_reads["reads"] += 1
                if truth(found.get("differ")):
                    tally.focused_reads["mismatches"] += 1
                if found.get("error"):
                    tally.focused_reads["failed"] += 1
            elif what == "hook_in":
                if hook_in is not None:
                    tally.unended_starts += 1
                tally.outpost_starts += 1
                hook_in = number(found.get("at_us"))
            elif what == "forwarding_ended" and hook_in is not None:
                tally.hold_us.append(number(found.get("at_us")) - hook_in)
                hook_in = None
        if hook_in is not None:
            tally.unended_starts += 1

    sent = {}
    for line in measure_lines(os.path.join(folder, "stderr.log")):
        if line.get("what") == "text_sent":
            sent[line.get("query")] = (line.get("op", "?"), number(line.get("at_us")))
        elif line.get("what") == "text_reduced" and line.get("query") in sent:
            op, at = sent.pop(line.get("query"))
            if op != "AwaitCaret":
                tally.text_us[op].append(number(line.get("at_us")) - at)
    return tally


def report(name, tally):
    out = [f"{name}: {tally.runs} runs, {tally.passed} passed"]
    for kind, label in (("uia", "UIA"), ("msaa", "MSAA")):
        counts = tally.focus[kind]
        out.append(
            f"  {label} focuses judged: {counts['focuses']}; repeats by NVDA's test: {counts['repeats']}; "
            f"of those current: {counts['current']}; Core did not hold: {counts['not held']}; "
            f"current and not held: {counts['current and not held']}; kept read never reached Core: "
            f"{counts['kept read never reached Core']}; dropped as a repeat by today's rule: "
            f"{counts['dropped as a repeat today']}"
        )
    menus = tally.menus
    out.append(
        f"  UIA menu openings: {menus['openings']}; with a focus of their application observed before "
        f"and applied after: {menus['with a focus observed before and applied after']}; dropped behind "
        f"a focus today: {menus['dropped behind a focus today']}; never reached Core: "
        f"{menus['never reached Core']}"
    )
    refusals = tally.refusals
    verdicts = ", ".join(f"{verdict} {count}" for verdict, count in sorted(tally.refusal_verdicts.items()))
    out.append(
        f"  focus reports Core reduced: {refusals['focus reports']}; not taken: {refusals['not taken']}; "
        f"refused while Core held another node of that application: "
        f"{refusals['while Core held another node of that application']}"
        + (f" ({verdicts})" if verdicts else "")
    )
    out.append(
        f"  outpost starts: {tally.outpost_starts}, {tally.unended_starts} with no forwarding end; "
        f"the hold's cost: {figures(tally.hold_us)}"
    )
    ratio = f"{sum(tally.gap_firings.values()) / tally.gap_copies:.3f}" if tally.gap_copies else "no copies"
    firings = "; ".join(
        f"uia_focus_before={before} sender_gone={gone}: {count}"
        for (before, gone), count in sorted(tally.gap_firings.items())
    )
    out.append(
        f"  WinEvent copies in a UIA window: {tally.gap_copies} ({tally.gap_copies_opened} opened a gap); "
        f"gap firings: {sum(tally.gap_firings.values())}, {ratio} per copy"
        + (f"; by field: {firings}" if firings else "")
    )
    reads = tally.focused_reads
    out.append(
        f"  outpost focused-element reads: {reads['reads']}; element in another process than the "
        f"checked window's provider: {reads['mismatches']}; failed, so not compared: {reads['failed']}"
    )
    every = [sample for samples in tally.text_us.values() for sample in samples]
    out.append(f"  text request round trip, caret watches left out: {figures(every)}")
    for op in sorted(tally.text_us):
        out.append(f"    {op}: {figures(tally.text_us[op])}")
    return "\n".join(out)


def main(roots):
    if not roots:
        print(__doc__.strip().splitlines()[0])
        print("usage: python -I tally.py <folder> [<folder> ...]")
        return 2
    by_scenario = defaultdict(Tally)
    for folder in run_folders(roots):
        by_scenario[scenario_of(folder)].add(tally_run(folder))
    total = Tally()
    for name in sorted(by_scenario):
        print(report(name, by_scenario[name]))
        total.add(by_scenario[name])
    print(report("total", total))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
