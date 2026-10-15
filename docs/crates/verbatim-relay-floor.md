# verbatim-relay-floor

The platform floor's benchmark for Verbatim's pipes and verbatim.exe's
relay (`docs/design/relay.md`, section 6; `docs/design/communication.md`,
section 11, step 2), a binary run by `cargo xtask relay-floor`;
`docs/tooling/measuring.md`, "Measuring the platform floor", says how to run it and
read it.

It has no library API. One executable plays every role. `run` is the
orchestrator, which is also the relay (verbatim.exe's stand-in) and
Core's synthesizer thread, and launches the others as contained
children with `verbatim_process::launch_with_ends`: over anonymous
pipes it makes itself with `CreatePipe`, synchronous at both ends, for
the variants that model the synchronous pipes Verbatim used before the
communication design (F1, F2 and F4), and over
`verbatim_process::overlapped` pipes for the ones that model the
design's. The child roles are
`--sender`, the listener's stand-in, whose producing threads each send a
burst of frames after a random gap waited out on a high-resolution
waitable timer; `--receiver`, the outpost's stand-in, which stamps each
frame as its read returns and reports every stamp as one frame when the
run ends; `--host`, a synthesizer host's stand-in, which answers each
command after a fixed burst of CPU; and `--drainer`, which writes one 2 MB
frame and exits once that write has completed.

The code that stands for the transport is in `wire.rs`: frames with a
readable length header (section 4.2), `FrameReader`, which keeps one
overlapped read in flight and asks for the whole rest of a large frame in
one read (section 4.1), and `Sender`, which writes one frame per write
with one write in flight, either from the producing thread with an I/O
thread finishing writes that pended, or with the I/O thread writing every
frame (section 4.3). `stats.rs` holds the percentiles and a counting
global allocator, so each process reports its allocations per frame.

The variants: F0, one wake-up; F1, one hop; F2, a relay that is one
synchronous thread; F3, a relay on a completion port with the children's
ends synchronous; F4, the relay as it was before stage A step 9 (reader
thread, channel, owner thread, condition variable, writer thread); F5,
F3's relay with the children's ends overlapped and the sender writing
inline; F6, a synthesizer host's round trip; and `frames`, reads per 1, 2
and 32 MB frame in both orders and the drain check. Every process opts
out of power throttling (`verbatim_process::scheduling`), and every
thread on the measured path runs at the priority asked for. Nothing is
logged or written on the measured path; the stamps stay in memory until
the run ends.
