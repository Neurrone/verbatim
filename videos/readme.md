# Videos

Demonstrations of Verbatim, each a recording of one end-to-end scenario
with Verbatim's own speech. The videos are stored with Git LFS
(`.gitattributes`), so a clone needs `git lfs install` once to get them
rather than pointers to them.

## Making one

Videos are added to this folder only when someone asks for one. Every
end-to-end run, CI's included, records each scenario to
`target/e2e-artifacts`, but nothing copies those here. On a Windows machine with ffmpeg on `PATH` and an
unlocked desktop:

`cargo xtask demo <scenario> [--name <name>]`

This builds and starts an agent of its own, runs the scenario through
the end-to-end harness while recording it, and saves the result as
`videos/<name>.mp4`. The name defaults to the scenario's, with hyphens
for underscores; `cargo xtask demo` with no arguments lists the
scenarios. A scenario that fails leaves this folder unchanged. Like any
local end-to-end run, it takes over the desktop while it runs
(`docs/tooling.md`).

A demo records at 30 frames a second, losslessly while the scenario
runs, and then encodes with a slow preset, so the encoding never slows
Verbatim down. The audio is exactly what Verbatim played, recorded by
Verbatim itself, so nothing else the machine plays ends up in it, and a
run is recorded the same way whether or not the computer has a sound
card.

## The videos

A line for each video: its file name, the scenario it records, and what
it demonstrates.

- `tabbing-through-settings.mp4`, from `menu_and_settings_dialog`:
  Verbatim opens its own context menu and Settings dialog and tabs
  through every control of the Speech page (the synthesizer, voice,
  variant, and the rate, pitch, inflection, and volume sliders, with a
  voice and a rate changed and changed back), each announced in full in
  eSpeak NG's voice. It demonstrates eSpeak NG as the default
  synthesizer, Verbatim reading its own interface, and a recording
  carrying Verbatim's speech.
