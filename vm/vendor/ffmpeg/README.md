# Vendored ffmpeg

`ffmpeg.exe`, which the end-to-end suite runs in the guest during
`cargo xtask vm test` to record each scenario's desktop video and to mux it
with the audio Verbatim records itself (see `crates/verbatim-e2e/src/recording.rs`).
`xtask vm test` points the suite at the guest's copy through
`VERBATIM_E2E_FFMPEG`. `ffprobe.exe`, from the same build, is no longer used
or deployed by anything. They are vendored here, at a pinned version,
rather than downloaded at build time: the ~100 MB downloads stalled or were
throttled inside the guest over Hyper-V's NAT, and Packer's own HTTP server
was not reachable from the guest through the host firewall. `xtask vm deploy`
copies `ffmpeg.exe` into the guest at `C:\VerbatimLab\tools` over PowerShell Direct
(VMBus), the same fast, reachability-free channel it uses for Verbatim's own
binaries, so it is no longer part of the golden-image build at all.

Because these are large binaries, they are stored with Git LFS (see the
repository's `.gitattributes`). A clone needs `git lfs` installed for the real
files to materialize; without it, the working tree holds LFS pointer files and
`xtask vm deploy` refuses to copy the pointer, with a message saying so.

## Pinned version

- ffmpeg 8.1.2, the `essentials` static build from gyan.dev
  (`8.1.2-essentials_build-www.gyan.dev`), a single self-contained executable
  with no side-by-side DLLs.
- `ffmpeg.exe` and the unused `ffprobe.exe` come from that same build.

## Updating

Download a newer gyan.dev `essentials` static build on the host, replace
`ffmpeg.exe` here, update this file's pinned-version line, and run one
`cargo xtask vm test --scenario <name>` to confirm that
`target/e2e-artifacts/<name>/<name>.mp4` still has both video and
Verbatim's speech. No image rebuild is required: the binary reaches the
guest at deploy time.
