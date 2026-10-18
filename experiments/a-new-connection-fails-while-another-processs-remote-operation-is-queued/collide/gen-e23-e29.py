# Writes the scripts of experiments e23 to e29 (section 20.8 of
# docs/design/focus-pipeline.md). Each runs the e1 pattern: L sets UIA up in
# S, S's UI thread is then busy without pumping, L sends a remote operation
# that waits for it, and 5 ms later B makes the call under test.
import sys

def write(name, comment, body):
    with open(name + ".txt", "w", encoding="utf-8") as f:
        f.write("".join("# " + line + "\n" for line in comment.splitlines()))
        f.write(body.strip() + "\ndrain 300\n")

SETUP_L = """
spawn L client $HWND L
send L init
wait L done
send L efh
wait L done
send L rop
wait L done 10000
"""

def busy_then(call, busy=40, b="B"):
    return f"""
send S busy {busy}
wait S busy-begin
send L async rop
sleep 5
send {b} {call}
wait {b} done 15000
wait L done 15000
"""

# e23: upward, same process. S: main window (shown off screen) and a child
# window, each with its own provider. B holds the child's element.
for call in ["parent", "normalize"]:
    write(f"e23-up-same-{call}",
          f"Upward crossing inside S: B holds S's child window's element and\n"
          f"calls {call} (into the main window's provider) while L's remote\n"
          f"operation into S waits for S's busy UI thread.",
          """
spawn S server
wait S hwnd
send S showoff
wait S shown
send S child
wait S hwnd
""" + SETUP_L + """
spawn B client $HWND2 B
send B init
wait B done
send B efh
wait B done
""" + busy_then(call))

# e24: upward, across processes. T's child window is parented under S's
# main window; B holds T's child's element; L's operation goes into S.
for call in ["parent", "normalize"]:
    write(f"e24-up-cross-{call}",
          f"Upward crossing between processes: T's child window is under S's\n"
          f"main window (as SystemSettings under ApplicationFrameHost). B holds\n"
          f"T's child's element and calls {call} (into S's provider) while L's\n"
          f"remote operation into S waits for S's busy UI thread.",
          """
spawn S server
wait S hwnd
send S showoff
wait S shown
spawn T server
wait T hwnd
send T childof $HWND
wait T hwnd
""" + SETUP_L + """
spawn B client $HWND3 B
send B init
wait B done
send B efh
wait B done
""" + busy_then(call))

# e25: downward, same process. B holds S's main window's element.
for call in ["firstchild", "children", "cachechildren", "descendants"]:
    write(f"e25-down-same-{call}",
          f"Downward crossing inside S: B holds S's main window's element and\n"
          f"calls {call}, reaching S's child window's provider, while L's\n"
          f"remote operation into S waits for S's busy UI thread.",
          """
spawn S server
wait S hwnd
send S showoff
wait S shown
send S child
wait S hwnd
""" + SETUP_L + """
spawn B client $HWND B
send B init
wait B done
send B efh
wait B done
""" + busy_then(call))

# e26: downward, across processes. S's child window is under T's main
# window; B holds T's main window's element; L's operation goes into S.
for call in ["firstchild", "children", "cachechildren"]:
    write(f"e26-down-cross-{call}",
          f"Downward crossing between processes: S's child window is under T's\n"
          f"main window. B holds T's element and calls {call}, reaching S's\n"
          f"child's provider, while L's remote operation into S waits for S's\n"
          f"busy UI thread.",
          """
spawn S server
wait S hwnd
spawn T server
wait T hwnd
send T showoff
wait T shown
send S childof $HWND2
wait S hwnd
""" + SETUP_L + """
spawn B client $HWND2 B
send B init
wait B done
send B efh
wait B done
""" + busy_then(call))

# e27: two remote operations from two processes. B has its own element of
# S's window, and has run a remote operation before (rops) or not (firstrop).
for kind in ["rops", "firstrop"]:
    before = "send B ropname\nwait B done 10000\n" if kind == "rops" else ""
    write(f"e27-two-rops-{kind}",
          "B's remote operation into S while L's waits for S's busy UI thread;\n"
          + ("B has run one into S before." if kind == "rops" else "B's first remote operation into S.")
          + "\nBoth read the element's name inside the operation.",
          """
spawn S server
wait S hwnd
spawn L client $HWND L
send L init
wait L done
send L efh
wait L done
send L ropname
wait L done 10000
spawn B client $HWND B
send B init
wait B done
send B efh
wait B done
""" + before + """
send S busy 40
wait S busy-begin
send L async ropname
sleep 5
send B ropname
wait B done 15000
wait L done 15000
""")

# e28: an idle UI thread of S while a remote operation into the busy main
# thread waits. B holds the element of the second thread's window. The busy
# lasts 1000 ms, so a delay behind it is plain.
for call in ["ropname", "name", "probe"]:
    write(f"e28-idle-thread-{call}",
          f"S has a second, idle UI thread. L's remote operation into S's main\n"
          f"window waits for the main thread, busy 1000 ms; B, holding the\n"
          f"second thread's window's element, calls {call} on it.",
          """
spawn S server
wait S hwnd
send S newthread
wait S hwnd
""" + SETUP_L + """
spawn B client $HWND2 B
send B init
wait B done
send B efh
wait B done
send B ropname
wait B done 10000
""" + busy_then(call, busy=1000))

# e29: MSAA during a queued remote operation. "only-root" makes S answer
# only UIA's own object id, so MSAA gets the system's standard proxy;
# without it, MSAA gets UIA's bridge to S's provider.
for server in ["bridge", "oleacc"]:
    flag = " --only-root" if server == "oleacc" else ""
    for call in ["msaa", "msaaevent"]:
        write(f"e29-msaa-{server}-{call}",
              f"B's first MSAA call into S ({call}; "
              + ("UIA's MSAA bridge to S's provider" if server == "bridge" else "the system's standard MSAA proxy")
              + ")\nwhile L's remote operation into S waits for S's busy UI thread.",
              f"""
spawn S server{flag}
wait S hwnd
""" + SETUP_L + """
spawn B client $HWND B
send B init
wait B done
""" + busy_then(call))
print("written")
