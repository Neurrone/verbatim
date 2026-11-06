"""PreToolUse hook: refuse a background shell command that waits without a time limit.

A background command that waits in a loop or follows a file for a line that never
comes runs forever, and the agent that started it waits for its notification; on
2026-11-06 two such waits idled a build for four hours. A wait must therefore end
by itself within 15 minutes and say so when it gives up.
"""

import json
import re
import sys

LIMIT_SECONDS = 900

# Commands that wait for something to happen.
WAITS = [
    (r"\buntil\b", "an `until` loop"),
    (r"\bwhile\b", "a `while` loop"),
    (r"\btail\b[^|;&]*\s(-\w*f\w*|--follow)\b", "`tail -f`"),
    (r"\bGet-Content\b[^|;]*-Wait\b", "`Get-Content -Wait`"),
    (r"\bWait-Process\b", "`Wait-Process`"),
    (r"\bWait-Event\b", "`Wait-Event`"),
    (r"\binotifywait\b", "`inotifywait`"),
]

# Ways a command bounds its own wait; the number, where there is one, is seconds.
LIMITS = [
    r"(?:^|[;&|(]\s*|\s)timeout\s+(?:-\w+\s+\S+\s+)*(\d+)(s?)\b",  # coreutils timeout 900
    r"-Timeout\s+(\d+)\b",  # Wait-Process / Wait-Event -Timeout 900
    r"\.WaitForExit\(\s*(\d+)\s*\)",  # milliseconds
]

MESSAGE = (
    "Refused: this background command waits ({what}) with no time limit. A wait whose "
    "condition never arrives runs forever and leaves whoever waits on it idle. Bound it "
    "to at most 15 minutes and make it say so when it gives up, for example in Bash: "
    "`timeout 900 bash -c '<your wait>' || echo \"TIMED OUT after 15 minutes waiting "
    "for <condition>\"`; in PowerShell: `Wait-Process -Id <pid> -Timeout 900`, or a loop "
    "on `$deadline = (Get-Date).AddMinutes(15)` that writes \"TIMED OUT after 15 minutes "
    "waiting for <condition>\" when it passes. Prefer waiting on the process itself over "
    "watching its output for a line."
)


def main() -> None:
    payload = json.load(sys.stdin)
    tool_input = payload.get("tool_input") or {}
    if not tool_input.get("run_in_background"):
        return
    command = tool_input.get("command") or ""
    waits = [what for pattern, what in WAITS if re.search(pattern, command, re.IGNORECASE)]
    if not waits:
        return
    what = ", ".join(waits)
    reason = None
    limits = []
    for index, pattern in enumerate(LIMITS):
        for match in re.finditer(pattern, command, re.IGNORECASE):
            value = int(match.group(1))
            limits.append(value / 1000 if index == 2 else value)
    has_deadline = re.search(r"\$deadline\b", command, re.IGNORECASE)
    if not limits and not has_deadline:
        reason = MESSAGE.format(what=what)
    elif limits and max(limits) > LIMIT_SECONDS:
        reason = (
            f"Refused: this background command waits ({what}) with a limit of "
            f"{int(max(limits))} s, over the 15 minutes allowed. Lower it to 900 s or less, "
            "and make the command say \"TIMED OUT after 15 minutes waiting for <condition>\" "
            "when it gives up."
        )
    if reason:
        print(
            json.dumps(
                {
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": reason,
                    }
                }
            )
        )


if __name__ == "__main__":
    main()
