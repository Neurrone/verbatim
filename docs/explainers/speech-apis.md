# The Windows speech synthesis APIs

Four generations of speech API coexist on Windows, and a screen reader
that wants users' preferred voices eventually meets all of them. This
file describes each on its own terms, deepest on SAPI 5 and SAPI 4
since Verbatim intends to consume both. Audio delivery concepts are in
[Audio](audio.md); how NVDA drives these APIs is in
[Synth drivers](../nvda/synth-drivers.md).

## What a screen reader needs from any of them

The checklist to judge each API against: sub-50 ms request-to-first-audio
at high speech rates; hard cancellation; *index marks* — callbacks when
playback (not synthesis) reaches marked points in the text, which is how
say-all cursors and spelling stay synchronized; rate/pitch/volume changes
per utterance without re-initialization; **PCM capture** — getting the
samples into your own audio path instead of letting the API play them,
so cancellation and mixing stay under your control; and crash isolation
proportional to how much you trust the engine.

## OneCore / WinRT speech

`Windows.Media.SpeechSynthesis` (Windows 10+): the modern voices. The
API is pull-oriented and passes the capture test trivially: you request
synthesis of text (or SSML) and receive a stream of PCM plus optional
word-boundary markers; you own playback entirely. Voice enumeration is
per-user installed voice packs. Weaknesses: no third-party engine
ecosystem (you get Microsoft's voices only), and latency that varies by
voice class — the "natural" neural voices miss the latency bar badly at
high rates. This is Verbatim's first driver.

## SAPI 5

The 2000-era COM API, and still the ecosystem where third-party
commercial voices ship. Two sides:

**Client side** (what a screen reader implements against):

- Voices are *tokens* in the registry
  (`HKLM\SOFTWARE\Microsoft\Speech\Voices\Tokens`, plus per-user and,
  for the newer OneCore-bridged voices,
  `...\Speech_OneCore\Voices\Tokens`); enumeration is
  `ISpObjectTokenCategory`/`SpObjectTokenCategory` over a category ID,
  each token carrying attributes (language, gender, vendor). This
  registry layout is why installing a voice needs an installer, and why
  32-bit and 64-bit registry views can expose different voice sets.
- The working interface is `ISpVoice` (CLSID `SpVoice`): `Speak` with
  flags (`SPF_ASYNC`, `SPF_PURGEBEFORESPEAK` — the cancellation
  primitive, XML parsing on or off), `SetRate` (a −10..10 log scale),
  `SetVolume`, `SetVoice(token)`, `Pause`/`Resume`.
- Markup: SAPI's own XML dialect (`<rate>`, `<pitch>`, `<spell>`,
  `<bookmark mark="..."/>`, `<lang>`), and SSML on engines that accept
  it. **Bookmarks are the index-mark mechanism**: interleave
  `<bookmark>` elements with text.
- Events arrive via `ISpEventSource`/`ISpNotifySink` (or window
  messages): start/end stream, word boundary, phoneme/viseme, and
  bookmark-reached. Caveat that shapes real drivers: events are
  delivered relative to the *audio output object's* clock — with the
  default output, timing tracks SAPI's own playback, which you are not
  using if you capture PCM.
- **PCM capture**: `ISpVoice::SetOutput` accepts any object
  implementing `ISpAudio` (an `IStream` extension with format
  negotiation and a clock). Implementing your own `ISpAudio` and
  handing it to `SetOutput` makes the engine "play" into your buffers,
  and makes *you* the clock that event timing follows — this is the
  load-bearing trick for integrating SAPI 5 into a screen reader's own
  audio pipeline (NVDA's implementation of exactly this:
  [Synth drivers](../nvda/synth-drivers.md)). The simpler
  `SPF_ASYNC`-into-`ISpStream`-over-memory approach also exists but
  loses live event timing.
- Threading: `SpVoice` is a free-threaded COM object; calls are safe
  from an MTA worker, but engines themselves vary in quality — the
  robust posture is one dedicated thread owning the voice.

**Engine side** (context for debugging engines): an engine implements
`ISpTTSEngine`; SAPI instantiates it *in the client's process*. A buggy
engine therefore crashes its host — the reason screen readers isolate
untrusted engines in a separate process. Bitness bites here: a 32-bit
engine DLL cannot load into a 64-bit process at all, so 32-bit-only
voices (common among older commercial voices) require a 32-bit host
process with PCM shipped across — the bridge pattern NVDA implements
(`sapi5_32`) and Verbatim's planned native synth host generalizes.

## SAPI 4

The 1996 predecessor, COM but a completely different object model.
Nothing modern implements it; it matters solely because beloved legacy
engines (Eloquence for SAPI 4, DECtalk lineage, older Keynote/RealSpeak
voices) exist only as SAPI 4 engines. The shape:

- Enumeration via `ITTSEnum` over installed engine *modes* (an engine
  exposes named voice modes with attributes); instantiation via
  `ITTSCentral` (per engine mode).
- Speaking is `ITTSCentral::TextData` — you hand it a text buffer and
  an `ITTSBufNotifySink` whose callbacks (`TextDataStarted`,
  `TextDataDone`, `BookmarkReached`) are the event channel; audio goes
  to an *audio destination* object (`IAudioDest` family), and
  implementing your own audio destination is the PCM-capture path,
  analogous to SAPI 5's `ISpAudio` (NVDA implements `IAudioDest`;
  see the `_sapi4.py` interface transcriptions cited in
  [Synth drivers](../nvda/synth-drivers.md)).
- Attributes (rate, pitch, volume) via `ITTSAttributes`, with
  per-engine ranges you must query rather than assume.
- Escapes: inline control codes (`\Mrk=n\` for bookmarks, `\Pit`,
  `\Spd`) rather than XML.
- **Everything is 32-bit only.** There was never a 64-bit SAPI 4, so
  any SAPI 4 support in a 64-bit world *requires* an out-of-process
  32-bit host from day one — on ARM64 that means an x86-emulated host
  process. Redistribution is also awkward: the SAPI 4 runtime is
  ancient and unmaintained; treat its presence on a user's system as
  their responsibility.

## Embedded engines

The fourth category is no API at all: engines linked or loaded
directly (eSpeak NG compiled into NVDA; Eloquence's `eci.dll` C API,
which screen readers historically call directly rather than through
SAPI 4). These are ordinary DLLs producing PCM on demand with
per-engine C APIs — near-zero latency, total control, zero isolation
unless you build a host process around them. Their latency profile
(milliseconds to first sample at extreme rates) is the bar long-time
screen reader users measure everything else against.

## References

- [Windows.Media.SpeechSynthesis namespace (Microsoft Learn)](https://learn.microsoft.com/en-us/uwp/api/windows.media.speechsynthesis)
- [SAPI 5.4 reference (Microsoft Learn, archived)](https://learn.microsoft.com/en-us/previous-versions/windows/desktop/ee125663(v=vs.85))
- [ISpVoice (Microsoft Learn, archived)](https://learn.microsoft.com/en-us/previous-versions/windows/desktop/ms719673(v=vs.85))
- SAPI 4: no online Microsoft documentation survives; the practical
  references are the SAPI 4 SDK headers (`speech.h`) and NVDA's
  interface transcriptions in `nvda/source/_synthDrivers32/_sapi4.py`.
