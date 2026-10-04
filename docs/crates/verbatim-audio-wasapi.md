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
event that the mixer's `Waker` signals.

Recovery. An `IMMNotificationClient` registered with the device
enumerator sets a flag and wakes the audio thread when the default render
device changes; `needs_reopen` reports the flag and the mixer reopens.
Any failure of the stream (an unplugged headset reports
`AUDCLNT_E_DEVICE_INVALIDATED`) is an error the mixer also answers by
reopening. `open` never fails: when no stream can be opened it falls
back to an internal `SilentDevice` that plays at real-time speed into
silence, and logs a warning. When the system has no render device at all
(`GetDefaultAudioEndpoint` returns `E_NOTFOUND`), the next default-device
change brings a real device back. When a device exists but failed to
open, which can happen briefly after a device change, `needs_reopen`
asks for another attempt a second later. The mixer reopens with its
shared state unlocked, so speech is never held up behind the device.
