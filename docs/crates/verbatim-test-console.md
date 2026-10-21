# verbatim-test-console

A console the tests start, for the tests of a console window's provider
process in `verbatim-uia` and `verbatim-outpost`
(`docs/design/listener-without-connections.md`, section 4.3). It is a
dev-dependency only: kept out of the crates it serves, so that their
tests and their builds compile them once, with the same features.

Public API:

- `Console::start()` starts `conhost.exe` running PowerShell in a hidden
  console window, in a job that ends both when the `Console` is dropped,
  and returns once the evidence that the client runs in it arrives: the
  window's title changed by the client to one unique to the test process,
  raised as a `WinEvent` from the console host. It panics when the console
  cannot be started or the title has not changed within ten seconds.
  PowerShell is the client because `cmd.exe`'s `title` command prefixes
  "Administrator: " when elevated, as GitHub's runner is.
- `Console::host`, the console host's process id, and `Console::window`,
  the console window, whose owner by `GetWindowThreadProcessId` is the
  client, PowerShell, while the host serves it.
- `Console::start_with_client_window()` starts the console as `start`
  does, its client first making a hidden Windows Forms window of its own
  on PowerShell's pipeline thread, another thread than the one Windows
  names as the console window's, and finds it by a title unique to the
  test process; it panics as `start` does, or when that window is not
  found. `Console::client_window` is that window, 0 for a console started
  with `start`. The moved-on rule's test in `verbatim-outpost` uses it.
