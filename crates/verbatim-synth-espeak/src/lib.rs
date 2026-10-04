//! The eSpeak NG synth driver (architecture section 6, decision D18).
//!
//! eSpeak NG is built from the vendored source and linked statically (see
//! `build.rs`); it is GPL version 3 or later, and runs only inside the
//! synthesizer host process, never in `verbatim.exe`. This driver is
//! written from eSpeak NG's public C API (`speak_lib.h`).
//!
//! eSpeak NG keeps its state in globals, so a process has one: the driver
//! refuses a second instance. It is used in synchronous mode: `espeak_Synth`
//! blocks and hands audio to a callback in chunks as it synthesizes, many
//! times faster than real time, and the callback's return value aborts
//! synthesis, which is how a cancel takes effect within one chunk.
//!
//! Index marks. eSpeak NG drops an SSML mark that follows a full stop
//! (up to at least master in September 2026), so the driver does not place
//! marks: it says [`SynthDriver::places_marks`] is false, and the speech
//! manager splits sequences at their marks, which keeps every mark exact
//! (decision D17). Text is passed as plain UTF-8, not SSML.

use std::ffi::{CStr, CString, c_char, c_int, c_uint, c_void};
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use verbatim_audio::PcmFormat;
use verbatim_speech::{
    SettingDescriptor, SettingId, SettingValue, SpeechItem, SpeechSequence, SynthDriver,
    SynthError, SynthId, SynthSink,
};

/// The stable id of the eSpeak NG driver.
pub const ESPEAK_ID: &str = verbatim_speech::hosting::synth_ids::ESPEAK;

/// The data directory's name, looked for next to the executable.
const DATA_DIR: &str = "espeak-ng-data";

/// The voice used until another is chosen: English.
const DEFAULT_VOICE: &str = "gmw/en";

/// The variant used until another is chosen, NVDA's default.
const DEFAULT_VARIANT: &str = "max";

/// The value of the variant setting that means no variant.
const NO_VARIANT: &str = "none";

/// Audio chunk length requested from eSpeak NG, in milliseconds: short, so
/// the first chunk arrives quickly and a cancel takes effect soon.
const CHUNK_MS: c_int = 20;

// From speak_lib.h.
const AUDIO_OUTPUT_SYNCHRONOUS: c_int = 2;
const ESPEAK_INITIALIZE_DONT_EXIT: c_int = 0x8000;
const ESPEAK_CHARS_UTF8: c_uint = 1;
/// `espeakSSML`: the text is SSML.
const ESPEAK_SSML: c_uint = 0x10;
const POS_CHARACTER: c_int = 1;
const EE_OK: c_int = 0;
const ESPEAK_RATE: c_int = 1;
const ESPEAK_VOLUME: c_int = 2;
const ESPEAK_PITCH: c_int = 3;
const ESPEAK_RANGE: c_int = 4;
const ESPEAK_RATE_MINIMUM: i32 = 80;
const ESPEAK_RATE_MAXIMUM: i32 = 450;

/// `espeak_EVENT`; only its type and user data are read here.
#[repr(C)]
struct EspeakEvent {
    kind: c_int,
    unique_identifier: c_uint,
    text_position: c_int,
    length: c_int,
    audio_position: c_int,
    sample: c_int,
    user_data: *mut c_void,
    id: [u8; 8],
}

/// `espeak_VOICE`.
#[repr(C)]
struct EspeakVoice {
    name: *const c_char,
    languages: *const c_char,
    identifier: *const c_char,
    gender: u8,
    age: u8,
    variant: u8,
    xx1: u8,
    score: c_int,
    spare: *mut c_void,
}

type SynthCallback = unsafe extern "C" fn(*mut i16, c_int, *mut EspeakEvent) -> c_int;

unsafe extern "C" {
    fn espeak_Initialize(
        output: c_int,
        buflength: c_int,
        path: *const c_char,
        options: c_int,
    ) -> c_int;
    fn espeak_SetSynthCallback(callback: SynthCallback);
    fn espeak_SetVoiceByName(name: *const c_char) -> c_int;
    fn espeak_SetParameter(parameter: c_int, value: c_int, relative: c_int) -> c_int;
    fn espeak_ListVoices(spec: *const EspeakVoice) -> *const *const EspeakVoice;
    fn espeak_Terminate() -> c_int;
    fn espeak_Synth(
        text: *const c_void,
        size: usize,
        position: c_uint,
        position_type: c_int,
        end_position: c_uint,
        flags: c_uint,
        unique_identifier: *mut c_uint,
        user_data: *mut c_void,
    ) -> c_int;
}

/// Whether this process already has an eSpeak NG driver.
static IN_USE: AtomicBool = AtomicBool::new(false);

/// One selectable voice or variant.
struct Choice {
    id: String,
    display_name: String,
}

/// The eSpeak NG synthesizer driver.
pub struct EspeakSynth {
    format: PcmFormat,
    voices: Vec<Choice>,
    variants: Vec<Choice>,
    voice: String,
    variant: String,
    rate: i32,
    pitch: i32,
    inflection: i32,
    volume: i32,
}

/// What the synthesis callback works on, for the length of one
/// `espeak_Synth` call.
struct Synthesis<'a> {
    sink: &'a mut dyn SynthSink,
    format: PcmFormat,
    stopped: bool,
}

/// Receives each chunk of audio from `espeak_Synth`.
unsafe extern "C" fn on_audio(wav: *mut i16, count: c_int, events: *mut EspeakEvent) -> c_int {
    // SAFETY: eSpeak NG passes the user data given to `espeak_Synth` in
    // every event, and the first event is always present; the pointer is to
    // the `Synthesis` alive for the whole synchronous `espeak_Synth` call.
    if events.is_null() {
        return 0;
    }
    let synthesis = unsafe {
        let user_data = (*events).user_data;
        if user_data.is_null() {
            return 0;
        }
        &mut *user_data.cast::<Synthesis<'_>>()
    };
    if synthesis.stopped {
        return 1;
    }
    let samples = match usize::try_from(count) {
        // SAFETY: `wav` holds `count` samples for the length of this call.
        Ok(count) if count > 0 && !wav.is_null() => unsafe {
            std::slice::from_raw_parts(wav, count)
        },
        _ => return 0,
    };
    match synthesis.sink.push_pcm(synthesis.format, samples) {
        ControlFlow::Continue(()) => 0,
        ControlFlow::Break(()) => {
            synthesis.stopped = true;
            1
        }
    }
}

/// The data directory: next to the executable when deployed, else where
/// this crate's build put it.
fn data_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(DATA_DIR)))
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| PathBuf::from(env!("VERBATIM_ESPEAK_BUILD_DATA")))
}

fn unavailable(detail: impl Into<String>) -> SynthError {
    SynthError::Unavailable(detail.into())
}

fn c_string(text: &str) -> Result<CString, SynthError> {
    CString::new(text)
        .map_err(|_| SynthError::Synthesis("text contains a NUL character".to_owned()))
}

/// Reads a C string eSpeak NG owns, lossily.
///
/// # Safety
///
/// `pointer` is null or a NUL-terminated string valid for the call.
unsafe fn read_c(pointer: *const c_char) -> String {
    if pointer.is_null() {
        String::new()
    } else {
        // SAFETY: as the caller guarantees.
        unsafe { CStr::from_ptr(pointer) }
            .to_string_lossy()
            .into_owned()
    }
}

/// Lists eSpeak NG's voices (`spec` null) or its variants (`spec` asking
/// for language "variant").
fn list(spec: Option<&EspeakVoice>) -> Vec<Choice> {
    let mut choices = Vec::new();
    // SAFETY: eSpeak NG is initialized; the returned array is
    // null-terminated and owned by eSpeak NG, read before the next call.
    unsafe {
        let mut entry = espeak_ListVoices(spec.map_or(std::ptr::null(), std::ptr::from_ref));
        while !entry.is_null() && !(*entry).is_null() {
            let voice = &**entry;
            choices.push(Choice {
                // eSpeak NG reports identifiers with the platform's path
                // separator (`gmw\en` on Windows); they are kept with `/`
                // so a saved setting means the same everywhere.
                id: read_c(voice.identifier).replace('\\', "/"),
                display_name: read_c(voice.name),
            });
            entry = entry.add(1);
        }
    }
    choices
}

/// Maps a 0 to 100 percent onto `min..=max`.
fn percent_to_param(percent: i32, min: i32, max: i32) -> i32 {
    min + (max - min) * percent / 100
}

impl EspeakSynth {
    /// Initializes eSpeak NG with the data next to the executable (or from
    /// the build), selecting English.
    ///
    /// # Errors
    ///
    /// Returns [`SynthError::Unavailable`] when this process already has an
    /// eSpeak NG driver, or eSpeak NG cannot start with its data.
    pub fn new() -> Result<Self, SynthError> {
        Self::with_data(&data_path())
    }

    /// Initializes eSpeak NG with the data directory at `data`.
    ///
    /// # Errors
    ///
    /// As [`EspeakSynth::new`].
    pub fn with_data(data: &Path) -> Result<Self, SynthError> {
        if IN_USE.swap(true, Ordering::AcqRel) {
            return Err(unavailable("this process already has an eSpeak NG driver"));
        }
        let result = Self::initialize(data);
        if result.is_err() {
            IN_USE.store(false, Ordering::Release);
        }
        result
    }

    fn initialize(data: &Path) -> Result<Self, SynthError> {
        // eSpeak NG falls back to other places (an environment variable,
        // another eSpeak NG's registry entry) when its path does not hold
        // data; this one must, so nothing else is ever loaded.
        if !data.join("phontab").is_file() {
            return Err(unavailable(format!(
                "eSpeak NG's data is missing from {}",
                data.display()
            )));
        }
        // eSpeak NG reads its path with narrow `fopen`, so it must be
        // representable; it accepts the data directory itself.
        let path = c_string(&data.to_string_lossy())
            .map_err(|_| unavailable("the eSpeak NG data path is not representable"))?;
        // SAFETY: a valid NUL-terminated path; DONT_EXIT makes a failure
        // return rather than end the process.
        let sample_rate = unsafe {
            espeak_Initialize(
                AUDIO_OUTPUT_SYNCHRONOUS,
                CHUNK_MS,
                path.as_ptr(),
                ESPEAK_INITIALIZE_DONT_EXIT,
            )
        };
        let sample_rate = u32::try_from(sample_rate)
            .ok()
            .filter(|rate| *rate > 0)
            .ok_or_else(|| {
                unavailable(format!(
                    "eSpeak NG could not start with its data at {}",
                    data.display()
                ))
            })?;
        // SAFETY: a valid callback for the life of the process.
        unsafe { espeak_SetSynthCallback(on_audio) };

        let variant_spec_language = c"variant";
        let variant_spec = EspeakVoice {
            name: std::ptr::null(),
            languages: variant_spec_language.as_ptr(),
            identifier: std::ptr::null(),
            gender: 0,
            age: 0,
            variant: 0,
            xx1: 0,
            score: 0,
            spare: std::ptr::null_mut(),
        };
        let mut variants = vec![Choice {
            id: NO_VARIANT.to_owned(),
            display_name: "None".to_owned(),
        }];
        variants.extend(list(Some(&variant_spec)).into_iter().map(|variant| {
            Choice {
                // Variant identifiers are paths such as `!v/max`; the name
                // eSpeak NG accepts after a `+` is the file name.
                id: variant
                    .id
                    .rsplit('/')
                    .next()
                    .unwrap_or(&variant.id)
                    .to_owned(),
                display_name: variant.display_name,
            }
        }));
        let mut synth = Self {
            format: PcmFormat {
                sample_rate,
                channels: 1,
            },
            voices: list(None),
            variants,
            voice: DEFAULT_VOICE.to_owned(),
            variant: DEFAULT_VARIANT.to_owned(),
            rate: 50,
            pitch: 50,
            inflection: 80,
            volume: 100,
        };
        if synth.voices.is_empty() {
            return Err(unavailable("eSpeak NG found no voices in its data"));
        }
        if !synth.voices.iter().any(|voice| voice.id == synth.voice) {
            synth.voice.clone_from(&synth.voices[0].id);
        }
        if !synth
            .variants
            .iter()
            .any(|variant| variant.id == synth.variant)
        {
            NO_VARIANT.clone_into(&mut synth.variant);
        }
        synth.apply_voice()?;
        synth.apply_parameters();
        Ok(synth)
    }

    /// Gives eSpeak NG the selected voice and variant.
    fn apply_voice(&self) -> Result<(), SynthError> {
        Self::select(&self.voice, &self.variant)
    }

    /// Gives eSpeak NG `voice` with `variant`.
    fn select(voice: &str, variant: &str) -> Result<(), SynthError> {
        let voice = voice.replace('/', std::path::MAIN_SEPARATOR_STR);
        let name = if variant == NO_VARIANT {
            voice
        } else {
            format!("{voice}+{variant}")
        };
        let name = c_string(&name)?;
        // SAFETY: a valid NUL-terminated name.
        match unsafe { espeak_SetVoiceByName(name.as_ptr()) } {
            EE_OK => Ok(()),
            error => Err(SynthError::Setting(format!(
                "eSpeak NG refused voice {}: error {error}",
                name.to_string_lossy()
            ))),
        }
    }

    /// Gives eSpeak NG the rate, pitch, inflection, and volume, which a
    /// voice change resets.
    fn apply_parameters(&self) {
        let parameters = [
            (
                ESPEAK_RATE,
                percent_to_param(self.rate, ESPEAK_RATE_MINIMUM, ESPEAK_RATE_MAXIMUM),
            ),
            (ESPEAK_PITCH, self.pitch),
            (ESPEAK_RANGE, self.inflection),
            (ESPEAK_VOLUME, self.volume),
        ];
        for (parameter, value) in parameters {
            // SAFETY: plain values within eSpeak NG's documented ranges.
            unsafe {
                espeak_SetParameter(parameter, value, 0);
            }
        }
    }
}

impl Drop for EspeakSynth {
    fn drop(&mut self) {
        // Initialize and terminate are eSpeak NG's supported cycle, so a
        // later driver in the same process starts from a clean state.
        // SAFETY: eSpeak NG was initialized by this driver, the only one.
        unsafe {
            espeak_Terminate();
        }
        IN_USE.store(false, Ordering::Release);
    }
}

/// Validates and stores a 0 to 100 percent setting.
fn set_percent(slot: &mut i32, value: i32) -> Result<(), SynthError> {
    if (0..=100).contains(&value) {
        *slot = value;
        Ok(())
    } else {
        Err(SynthError::Setting(format!(
            "value {value} outside 0..=100"
        )))
    }
}

impl SynthDriver for EspeakSynth {
    fn id(&self) -> SynthId {
        SynthId::new(ESPEAK_ID)
    }

    fn display_name(&self) -> String {
        "eSpeak NG".to_owned()
    }

    fn supported_settings(&self) -> Vec<SettingDescriptor> {
        let options = |choices: &[Choice]| {
            choices
                .iter()
                .map(|choice| (choice.id.clone(), choice.display_name.clone()))
                .collect()
        };
        vec![
            SettingDescriptor::Choice {
                id: SettingId::new("voice"),
                label_key: "setting-voice".to_owned(),
                options: options(&self.voices),
            },
            SettingDescriptor::Choice {
                id: SettingId::new("variant"),
                label_key: "setting-variant".to_owned(),
                options: options(&self.variants),
            },
            SettingDescriptor::standard_numeric("rate", "setting-rate"),
            SettingDescriptor::standard_numeric("pitch", "setting-pitch"),
            SettingDescriptor::standard_numeric("inflection", "setting-inflection"),
            SettingDescriptor::standard_numeric("volume", "setting-volume"),
        ]
    }

    fn setting(&self, id: &SettingId) -> Option<SettingValue> {
        match id.0.as_str() {
            "voice" => Some(SettingValue::Choice(self.voice.clone())),
            "variant" => Some(SettingValue::Choice(self.variant.clone())),
            "rate" => Some(SettingValue::Number(self.rate)),
            "pitch" => Some(SettingValue::Number(self.pitch)),
            "inflection" => Some(SettingValue::Number(self.inflection)),
            "volume" => Some(SettingValue::Number(self.volume)),
            _ => None,
        }
    }

    fn set_setting(&mut self, id: &SettingId, value: SettingValue) -> Result<(), SynthError> {
        match (id.0.as_str(), value) {
            ("voice", SettingValue::Choice(choice)) => {
                if !self.voices.iter().any(|voice| voice.id == choice) {
                    return Err(SynthError::Setting(format!("unknown voice {choice}")));
                }
                // Stored only once eSpeak NG has taken it, so a refused
                // voice never becomes the one reported or restored.
                Self::select(&choice, &self.variant)?;
                self.voice = choice;
            }
            ("variant", SettingValue::Choice(choice)) => {
                if !self.variants.iter().any(|variant| variant.id == choice) {
                    return Err(SynthError::Setting(format!("unknown variant {choice}")));
                }
                Self::select(&self.voice, &choice)?;
                self.variant = choice;
            }
            ("rate", SettingValue::Number(number)) => set_percent(&mut self.rate, number)?,
            ("pitch", SettingValue::Number(number)) => set_percent(&mut self.pitch, number)?,
            ("inflection", SettingValue::Number(number)) => {
                set_percent(&mut self.inflection, number)?;
            }
            ("volume", SettingValue::Number(number)) => set_percent(&mut self.volume, number)?,
            (other, _) => {
                return Err(SynthError::Setting(format!(
                    "unknown or mistyped setting {other}"
                )));
            }
        }
        self.apply_parameters();
        Ok(())
    }

    fn changes_pitch(&self) -> bool {
        true
    }

    fn places_marks(&self) -> bool {
        false
    }

    fn speak(
        &mut self,
        sequence: &SpeechSequence,
        sink: &mut dyn SynthSink,
    ) -> Result<(), SynthError> {
        let plain: String = sequence
            .items
            .iter()
            .filter_map(|item| match item {
                SpeechItem::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        if plain.trim().is_empty() || sink.is_cancelled() {
            return Ok(());
        }
        // A pitch change is spoken within the one synthesis, as SSML, as
        // NVDA's eSpeak NG driver speaks it; otherwise the text is plain.
        let (text, flags) = if sequence.has_pitch_changes() {
            (
                ssml_with_pitch(sequence, self.pitch),
                ESPEAK_CHARS_UTF8 | ESPEAK_SSML,
            )
        } else {
            (plain, ESPEAK_CHARS_UTF8)
        };
        let text = c_string(&text)?;
        let mut synthesis = Synthesis {
            sink,
            format: self.format,
            stopped: false,
        };
        // SAFETY: `text` is NUL-terminated UTF-8 alive for the call;
        // `synthesis` outlives it, and the synchronous call delivers every
        // callback before it returns.
        let result = unsafe {
            espeak_Synth(
                text.as_ptr().cast(),
                text.as_bytes_with_nul().len(),
                0,
                POS_CHARACTER,
                0,
                flags,
                std::ptr::null_mut(),
                (&raw mut synthesis).cast(),
            )
        };
        match result {
            EE_OK => Ok(()),
            error => Err(SynthError::Synthesis(format!(
                "eSpeak NG failed: error {error}"
            ))),
        }
    }
}

/// The sequence as SSML for eSpeak NG, its text escaped and each pitch
/// change a `prosody` element, as NVDA's driver writes it: the new pitch as
/// a percentage of the configured `pitch` (0 to 100), so 50 raised by 30
/// is 160%. Marks are left out; this driver does not place them.
fn ssml_with_pitch(sequence: &SpeechSequence, pitch: i32) -> String {
    use std::fmt::Write as _;
    let mut ssml = String::new();
    let mut open = false;
    for item in &sequence.items {
        match item {
            SpeechItem::Text(text) => ssml.push_str(&escape_xml(text)),
            SpeechItem::Pitch(offset) => {
                if open {
                    ssml.push_str("</prosody>");
                    open = false;
                }
                if *offset != 0 {
                    let percent = (pitch + offset).clamp(0, 100) * 100 / pitch.max(1);
                    let _ = write!(ssml, "<prosody pitch=\"{percent}%\">");
                    open = true;
                }
            }
            _ => {}
        }
    }
    if open {
        ssml.push_str("</prosody>");
    }
    ssml
}

/// Escapes the three characters SSML gives meaning to.
fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pitch_change_is_a_prosody_element_relative_to_the_pitch_setting() {
        let sequence = SpeechSequence {
            utterance: verbatim_model::UtteranceId(1),
            trace_id: verbatim_model::TraceId::mint(),
            language: None,
            items: vec![
                SpeechItem::Text("a & ".to_owned()),
                SpeechItem::Pitch(30),
                SpeechItem::Text("B".to_owned()),
                SpeechItem::Pitch(0),
                SpeechItem::Text(" c".to_owned()),
            ],
        };
        assert_eq!(
            ssml_with_pitch(&sequence, 50),
            "a &amp; <prosody pitch=\"160%\">B</prosody> c"
        );
    }

    #[test]
    fn rate_percent_spans_espeak_ng_s_word_rates() {
        assert_eq!(
            percent_to_param(0, ESPEAK_RATE_MINIMUM, ESPEAK_RATE_MAXIMUM),
            80
        );
        assert_eq!(
            percent_to_param(100, ESPEAK_RATE_MINIMUM, ESPEAK_RATE_MAXIMUM),
            450
        );
    }
}
