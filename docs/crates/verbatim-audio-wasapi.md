# verbatim-audio-wasapi

The WASAPI implementation of [verbatim-audio](verbatim-audio.md)'s
`AudioDevice` seam (decisions D5 and D17), which the mixer writes to. It
is a separate crate so that the mixer, and everything coded against it,
carries no Windows dependency.

Public API: `WasapiDevice`, constructed with `new`, which opens nothing;
the mixer's audio thread calls `open`, so COM and the stream live on that
thread.

Implementation notes: an event-driven shared-mode stream on the default
render endpoint (console role), in the endpoint's own mix rate and
channel layout (the mix format's channel mask is kept) as 32-bit float.
The mixer converts every source to that format, so the stream needs no
further conversion. The requested buffer is about 40 ms; the mixer
counts underruns to check it is not too small. COM is initialized on the
audio thread as MTA, tolerating `RPC_E_CHANGED_MODE`, and the audio
thread is registered with the Multimedia Class Scheduler Service as a
"Pro Audio" task (decision D15), so it is scheduled ahead of ordinary
work while Verbatim or the system is busy.

`queued_frames` is the stream's current padding, `write` copies into the
render buffer, `start` starts the stream, and `stop` issues Stop plus
Reset to discard everything queued, which is how speech interruption
sounds instant. `wait` blocks on the stream's render event and a wake
event that the mixer's `Waker` signals. `clock_position` reads the
stream's `IAudioClock` position, converted to frames, and how long ago it
was taken, from the performance counter's value the position carries.

Recovery. An `IMMNotificationClient`, registered once with the device
enumerator on the audio thread, passes Windows' endpoint notifications
to the device's `DeviceNotices` (see [verbatim-audio](verbatim-audio.md)):
a default render device change (console role) as `DefaultChanged`, and a
device added or changed to the active state as `Available`, not filtered
by data flow, since that would take a call into the enumerator from the
callback, and an arrival costs a reopen only while an open has failed.
`needs_reopen` reports the notices, and a notice that asks for a reopen
signals the wait's event itself. Any failure of the stream (an unplugged
headset reports `AUDCLNT_E_DEVICE_INVALIDATED`) is an error the mixer
also answers by reopening.

When no stream can be opened, `open` returns the error, and the mixer
plays real-time silence in the device's place until `needs_reopen` asks.
What the next open waits for depends on the failure (`open_failure`):
when the system has no render device (`GetDefaultAudioEndpoint` returns
`E_NOTFOUND`), only a notification, with no timer. Any other failure can
come from a device that is still there, and Windows sends no
notification when such a failure clears, so the device also asks a
second later, and `wait` ends by then. That covers an active device
refusing the stream, such as another program holding it in exclusive
mode (`AUDCLNT_E_DEVICE_IN_USE`; checked on 2026-10-21 by holding the
device exclusively and releasing it, which raised no notification), the
audio service not running, or a resource the engine lacked. It covers
`AUDCLNT_E_DEVICE_INVALIDATED` too, which reports a device reconfigured
in place as well as one that went away: changing the default device's
format failed an open made while the engine reconfigured, and Windows
raised only property changes, none after that open returned (checked on
2026-10-21; `docs/design/communication.md`, section 16). A failure also
waits for the deadline when the notification client could not be
registered. The mixer reopens with
its shared state unlocked, so speech is never held up behind the device.
