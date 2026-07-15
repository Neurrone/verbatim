# Vendored ffmpeg

`ffmpeg.exe` and `ffprobe.exe` used by `cargo xtask vm test --record` to
record the guest desktop (with VB-CABLE loopback audio) and to probe the
result for an audio stream. They are vendored here, at a pinned version,
rather than downloaded at build time: the ~100 MB downloads stalled or were
throttled inside the guest over Hyper-V's NAT, and Packer's own HTTP server
was not reachable from the guest through the host firewall. `xtask vm deploy`
copies both into the guest at `C:\VerbatimLab\tools` over PowerShell Direct
(VMBus), the same fast, reachability-free channel it uses for Verbatim's own
binaries, so they are no longer part of the golden-image build at all.

Because these are large binaries, they are stored with Git LFS (see the
repository's `.gitattributes`). A clone needs `git lfs` installed for the real
files to materialize; without it, the working tree holds LFS pointer files and
`--record` runs will fail to launch ffmpeg.

## Pinned version

- ffmpeg 8.1.2, the `essentials` static build from gyan.dev
  (`8.1.2-essentials_build-www.gyan.dev`), a single self-contained executable
  with no side-by-side DLLs.
- Both `ffmpeg.exe` and `ffprobe.exe` come from that same build.

## Updating

Download a newer gyan.dev `essentials` static build on the host, replace both
executables here, update this file's pinned-version line, and run one
`cargo xtask vm test --record` to confirm capture and the audio probe still
work. No image rebuild is required — the binaries reach the guest at deploy
time.
