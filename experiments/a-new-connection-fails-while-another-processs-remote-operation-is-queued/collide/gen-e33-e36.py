"""Writes the scripts for e33 to e36: the classic-listener experiments.

In each, L (process A) has set UIA up in the application under test and
then sends a remote operation into it while the application's UI thread
is busy (S: the server's own busy loop; the console: its window thread
suspended), so the operation stays queued; B (a separate process) makes
the call under test 5 ms later. A control script for each omits L's
operation and keeps everything else.
"""

import sys
from pathlib import Path

HERE = Path(__file__).parent
BUSY = 300

SERVER = """spawn S server
wait S hwnd
send S showtopoff
wait S shown
owner $HWND
"""

L_SETUP = """spawn L client $HWND L
send L init
wait L done
send L efh
wait L done
send L rop
wait L done 10000
"""


def stall_s(rop):
    out = f"send S busy {BUSY}\nwait S busy-begin\n"
    if rop:
        out += "send L async rop\n"
    return out + "sleep 5\n"


def stall_console(rop):
    out = f"stall tid:${{CONSOLE_TID}} {BUSY}\n"
    if rop:
        out += "send L async rop\n"
    return out + "sleep 5\n"


def write(name, header, body):
    (HERE / f"{name}.txt").write_text(header + body + "drain 500\n", encoding="utf-8")


def exp1(rop):
    """B walks down from the desktop root to S's window, then reads it."""
    tag = "" if rop else "-control"
    for command in ["rootwalk", "rootwalkcv", "rootfind", "rootfindall", "efhbc"]:
        header = (
            "# e33: B, a new process that has never connected to S, reaches\n"
            f"# S's top-level window by {command} from the desktop root while\n"
            "# L's remote operation waits in S's busy UI thread"
            + ("" if rop else " (control: no operation)")
            + ", then reads\n# the element found (info). efhbc is the known-failing call, as a\n"
            "# positive control for the collision under these timings.\n"
        )
        body = SERVER + L_SETUP + "spawn B client $HWND B\nsend B init\nwait B done\n"
        body += stall_s(rop)
        body += f"send B {command}\nwait B done 15000\nsend B info\nwait B done 15000\n"
        write(f"e33-root-{command}{tag}", header, body)


def exp2(rop):
    """B's raw WM_GETOBJECT probe beside C's UiaHasServerSideProvider."""
    tag = "" if rop else "-control"
    header = (
        "# e34: during L's queued remote operation in S"
        + ("" if rop else " (control: no operation)")
        + ", B sends a raw\n# WM_GETOBJECT(UiaRootObjectId) to S's window and C, another new\n"
        "# process, calls UiaHasServerSideProvider on it at the same moment.\n"
        "# Then both ask the same of N, a window without a UIA provider.\n"
    )
    body = (
        SERVER
        + "spawn N server --no-uia\nwait N hwnd\nowner $HWND2\n"
        + L_SETUP
        + "spawn B client $HWND B\nsend B init\nwait B done\n"
        + "spawn C client $HWND C\nsend C init\nwait C done\n"
        + stall_s(rop)
        + "send B rawprobe\nsend C probe\nwait B done 15000\nwait C done 15000\n"
        + "drain 300\n"
        + "send B target $HWND2\nwait B done\nsend B rawprobe\nwait B done 15000\n"
        + "send C target $HWND2\nwait C done\nsend C probe\nwait C done 15000\n"
    )
    write(f"e34-rawprobe{tag}", header, body)


def exp3_console(rop):
    """B walks up from the console's text area to the console window."""
    tag = "" if rop else "-control"
    header = (
        "# e35: a conhost console (cmd.exe as its client, titled\n"
        "# collide-console, started before the run). B holds the console's\n"
        "# text area (efh on the window, then the document under it) and walks\n"
        "# up, parent by parent, to the element under the desktop root while\n"
        "# L's remote operation into the console waits for the console's\n"
        "# window thread (conhost's, given as CONSOLE_TID; the window itself\n"
        "# reports cmd.exe as its owner), suspended for the purpose"
        + ("" if rop else " (control: no operation)")
        + ".\n"
    )
    body = (
        "findwindow collide-console\nowner $HWND\n"
        + L_SETUP
        + "spawn B client $HWND B\nsend B init\nwait B done\nsend B efh\nwait B done\n"
        + "send B docchild\nwait B done\n"
        + stall_console(rop)
        + "send B toplevel\nwait B done 15000\nsend B info\nwait B done 15000\n"
    )
    write(f"e35-console-up{tag}", header, body)
    header = (
        "# e35: positive control for the console: B, a new process, calls\n"
        "# ElementFromHandleBuildCache on the console window while L's remote\n"
        "# operation waits for the suspended window thread"
        + ("" if rop else " (control: no operation)")
        + ".\n"
    )
    body = (
        "findwindow collide-console\nowner $HWND\n"
        + L_SETUP
        + "spawn B client $HWND B\nsend B init\nwait B done\n"
        + stall_console(rop)
        + "send B efhbc\nwait B done 15000\nsend B info\nwait B done 15000\n"
    )
    write(f"e35-console-efhbc{tag}", header, body)


def exp3_own(rop):
    """B walks up from S's child window, found under a held S element."""
    tag = "" if rop else "-control"
    header = (
        "# e36: B holds S's top-level element (efh before the operation)\n"
        "# and the element of S's child window under it (first child), then\n"
        "# walks up from the child to the element under the desktop root while\n"
        "# L's remote operation waits in S's busy UI thread"
        + ("" if rop else " (control: no operation)")
        + ".\n"
    )
    body = (
        SERVER
        + "send S child\nwait S hwnd\n"
        + L_SETUP
        + "spawn B client $HWND B\nsend B init\nwait B done\nsend B efh\nwait B done\n"
        + "send B holdfirstchild\nwait B done\n"
        + stall_s(rop)
        + "send B toplevel\nwait B done 15000\nsend B info\nwait B done 15000\n"
    )
    write(f"e36-own-up{tag}", header, body)


if __name__ == "__main__":
    for rop in (True, False):
        exp1(rop)
        exp2(rop)
        exp3_console(rop)
        exp3_own(rop)
    print("written", file=sys.stderr)
