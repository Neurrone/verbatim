"""One line per blocked wait of a `cargo xtask trace waits --blocked` report.

blocked-summary.py <report> <image> <pid> <from> <to>

Prints, for each wait of the process's threads that overlaps the window
(trace times, such as 08:10:25.0), its start, thread, length, what woke it,
and the UIA, oleacc, user32 and mockapp frames of the stack it waited on (W)
and of the stack that readied it (R), innermost first.
"""

import re
import sys

KEYS = re.compile(
    r"RealCrossProcess_\w+|GetComboBoxInfoTimeout|DoNonClientNavigation|AccessibleObjectFromWindowTimeout"
    r"|SendMessageTimeoutW|RequestResponseCoordinator::WaitForResponse|ChannelBasedClientConnection::WaitForResponse"
    r"|FindingVisitor::\w+|HwndProxy::\w+|MsaaNonClientProxyRoot::\w+|ElementFinder::Find|CAutomationElement::\w+"
    r"|OverlappedIOManager::\w+|ChannelBasedServerConnection::\w+|ComInvoker::CallTarget|MTAThreadWaitForCall"
    r"|ProcessIncomingBatchedMessages|ReleaseCollection::\w+|mockapp::window::\w+|GetMessageW|DispatchMessageWorker"
    r"|WriteFile|ReadFile"
)


def frames(lines):
    seen = []
    for line in lines:
        key = KEYS.search(line)
        if key and key.group(0) not in seen:
            seen.append(key.group(0))
    return " < ".join(seen)


def main():
    report, image, pid, start, end = sys.argv[1:6]
    head = re.compile(
        re.escape(image) + r" (\d+) thread (\d+) \"[^\"]*\" blocked (\d+) ms, from (\S+) to (\S+), woken by (.*)"
    )
    waits, current, section = [], None, None
    for line in open(report, encoding="utf-8-sig").read().split("\n"):
        if re.match(r"^\S+\.exe \d+ thread", line):
            if current:
                waits.append(current)
            current, section = [line, [], []], None
        elif "where it waited" in line:
            section = 1
        elif "where it was readied" in line:
            section = 2
        elif current and section:
            current[section].append(line.strip())
    if current:
        waits.append(current)
    out = []
    for header, waited, readied in waits:
        m = head.match(header)
        if not m or m.group(1) != pid or m.group(5) < start or m.group(4) > end:
            continue
        out.append(
            f"{m.group(4)} th{m.group(2)} {m.group(3):>5} ms woken by {m.group(6)[:40]}"
            f" | W: {frames(waited)} | R: {frames(readied)}"
        )
    print("\n".join(sorted(out)))


main()
