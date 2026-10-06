//! The presentation stage (decision D12): the pipeline boundary where a
//! [`Presenter`] flattens a structured [`Utterance`] into a flat
//! [`SpeechSequence`], as the active theme says (`phase6-design.md`,
//! "Themes: one model for verbosity, speech, and sounds").
//!
//! The reducer composes announcements from semantic spans, never display
//! text, so localization and presentation both happen here. Each span that
//! is an indication of the catalogue (a role, a state, a description, a
//! spelling error) is reported as the theme says: in words, by a sound
//! placed in the speech stream at the span's place, both, or not at all. A
//! span that is content (a label, a value, text) is always spoken.
//! [`ThemePresenter`] does this with the theme a [`ThemeHandle`] holds,
//! which can be switched while speech runs; with the built-in default theme
//! and no sounds it speaks as NVDA does.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};

use tracing::warn;
use verbatim_audio::Sound;
use verbatim_i18n::{
    character_description, character_name, format_text, level, message_text, negated_state_name,
    phrase_text, position_in_set, role_name, state_name,
};
use verbatim_model::{
    Earcon, Fetches, Indication, IndicationSetting, MAX_GAIN, Presentation, SegmentContent,
    SoundSource, TextFormat, Theme, ThemeOptions, ThemeProblem, Utterance, UtteranceId,
    progress_frequency,
};

use crate::driver::{IndexMark, SoundCue, SpeechItem, SpeechSequence};

/// The presentation stage: flattens structured utterances at the end of the
/// speech pipeline.
///
/// Implementations run on the pipeline's queue thread (hence `Send`) and
/// must be cheap: this sits between "utterance queued" and "synthesis
/// starts" on every spoken announcement, inside the latency budget.
pub trait Presenter: Send {
    /// Flattens one structured utterance, which the pipeline has numbered
    /// `id`, to the sequence handed to the synthesizer.
    fn flatten(&self, utterance: &Utterance, id: UtteranceId) -> SpeechSequence;
}

/// How far the pitch setting is raised for a capital letter spelled out:
/// NVDA's default `capPitchChange`, which is not yet configurable here.
pub const CAPITAL_PITCH_OFFSET: i32 = 30;

/// A theme ready to present with: every indication's setting resolved
/// against the default theme, its sounds decoded, and the settings that go
/// with it.
#[derive(Clone, Debug)]
pub struct ActiveTheme {
    theme: Theme,
    settings: HashMap<Indication, IndicationSetting>,
    sounds: HashMap<SoundSource, Arc<Sound>>,
    options: ThemeOptions,
    problems: Vec<ThemeProblem>,
}

impl Default for ActiveTheme {
    /// The built-in default theme with no sounds: everything is spoken,
    /// since an indication whose sound is unavailable is spoken instead.
    fn default() -> Self {
        Self::new(Theme::builtin_default(), |_| None, ThemeOptions::default())
    }
}

impl ActiveTheme {
    /// Makes `theme` ready: resolves each of the catalogue's indications,
    /// decodes every sound file it names, found through `sound_path` (a file
    /// name to the file, `None` when there is none), and generates its
    /// tones. A sound that is missing or cannot be decoded is reported in
    /// [`problems`](Self::problems) and logged, and the indications using
    /// it are spoken instead.
    pub fn new(
        theme: Theme,
        sound_path: impl Fn(&str) -> Option<PathBuf>,
        options: ThemeOptions,
    ) -> Self {
        let settings: HashMap<Indication, IndicationSetting> = Indication::catalogue()
            .into_iter()
            .chain(theme.indications.keys().copied())
            .map(|indication| (indication, theme.setting(indication)))
            .collect();
        let mut sounds = HashMap::new();
        let mut problems = Vec::new();
        let mut used: Vec<(&Indication, &SoundSource)> = settings
            .iter()
            .filter_map(|(indication, setting)| Some((indication, setting.sound.as_ref()?)))
            .collect();
        used.sort();
        for (indication, source) in used {
            if sounds.contains_key(source) {
                continue;
            }
            let decoded = match source {
                SoundSource::Tone(tone) => Sound::tone(tone.frequency_hz, tone.duration_ms)
                    .map_err(|error| (format!("{}Hz", tone.frequency_hz), error.to_string())),
                SoundSource::File(file) => {
                    let Some(path) = sound_path(file) else {
                        problems.push(ThemeProblem::MissingSound {
                            indication: *indication,
                            file: file.clone(),
                        });
                        continue;
                    };
                    Sound::from_wav_file(&path).map_err(|error| (file.clone(), error.to_string()))
                }
            };
            match decoded {
                Ok(sound) => {
                    sounds.insert(source.clone(), Arc::new(sound));
                }
                Err((file, reason)) => problems.push(ThemeProblem::UnreadableSound {
                    indication: *indication,
                    file,
                    reason,
                }),
            }
        }
        for problem in &problems {
            warn!(target: "verbatim::speech", theme = %theme.id, %problem, "theme sound unavailable");
        }
        Self {
            theme,
            settings,
            sounds,
            options,
            problems,
        }
    }

    /// The theme.
    #[must_use]
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// The settings that go with it.
    #[must_use]
    pub fn options(&self) -> ThemeOptions {
        self.options
    }

    /// The same theme with other settings, keeping its decoded sounds.
    #[must_use]
    pub fn with_options(&self, options: ThemeOptions) -> Self {
        Self {
            options,
            ..self.clone()
        }
    }

    /// The sounds that could not be loaded.
    #[must_use]
    pub fn problems(&self) -> &[ThemeProblem] {
        &self.problems
    }

    /// What the reducer fetches under this theme.
    #[must_use]
    pub fn fetches(&self) -> Fetches {
        self.theme.fetches()
    }

    /// How `indication` is reported now: its sound, when it plays one and
    /// the sound is available, and whether it is spoken. An indication set
    /// to sound alone whose sound is unavailable is spoken instead, and so
    /// is one whose sound plays when the settings ask for sounded
    /// indications to be spoken too.
    fn decide(&self, indication: Indication) -> Decision<'_> {
        let setting = self.setting(indication);
        let sound = setting
            .report
            .sounds()
            .then_some(setting.sound.as_ref())
            .flatten()
            .and_then(|source| Some((source, self.sounds.get(source)?)));
        let speak = match setting.report {
            Presentation::Off => false,
            Presentation::Speech | Presentation::SpeechAndSound => true,
            Presentation::Sound => sound.is_none() || self.options.speak_sounded_indications,
        };
        Decision {
            setting,
            sound,
            gain: self.gain(setting),
            speak,
        }
    }

    /// The resolved setting of `indication`.
    fn setting(&self, indication: Indication) -> &IndicationSetting {
        static SPEECH: std::sync::LazyLock<IndicationSetting> =
            std::sync::LazyLock::new(IndicationSetting::default);
        self.settings.get(&indication).unwrap_or(&SPEECH)
    }

    /// The gain a sound of `setting` plays at: the theme's, the
    /// indication's, and the sound volume together.
    fn gain(&self, setting: &IndicationSetting) -> f32 {
        let percent = |gain: u16| f32::from(gain.min(MAX_GAIN)) / 100.0;
        percent(self.theme.gain)
            * percent(setting.gain)
            * f32::from(self.options.sound_volume.min(100))
            / 100.0
    }

    /// The pitch change of the voice style `setting` names, 0 for none.
    fn voice_pitch(&self, setting: &IndicationSetting) -> i32 {
        setting
            .voice
            .as_ref()
            .and_then(|name| self.theme.voice_styles.get(name))
            .map_or(0, |style| style.pitch.clamp(-100, 100))
    }

    /// The sound and words of an event the theme reports at once
    /// (`SpeechManager::play_earcon`): the sound, with its gain, to play
    /// now, and the words to speak, each `None` when the theme says not
    /// to. A progress tone rises in pitch with the percentage.
    pub(crate) fn earcon(&self, earcon: Earcon) -> (Option<(Arc<Sound>, f32)>, Option<String>) {
        let indication = Indication::of_earcon(earcon);
        let decision = self.decide(indication);
        let sound = decision
            .sound
            .and_then(|(source, sound)| match (earcon, source) {
                (Earcon::Progress(percent), SoundSource::Tone(tone)) => Sound::tone(
                    progress_frequency(tone.frequency_hz, percent),
                    tone.duration_ms,
                )
                .ok()
                .map(Arc::new),
                _ => Some(Arc::clone(sound)),
            });
        let words = decision.speak.then(|| {
            words_with(
                decision.setting.words.as_deref(),
                Some(verbatim_i18n::earcon_text(earcon)),
                matches!(earcon, Earcon::Progress(_)),
            )
        });
        (sound.map(|sound| (sound, decision.gain)), words.flatten())
    }
}

/// How one indication is reported now.
struct Decision<'a> {
    setting: &'a IndicationSetting,
    sound: Option<(&'a SoundSource, &'a Arc<Sound>)>,
    gain: f32,
    speak: bool,
}

/// A shared, switchable handle on the active theme: the presenter reads it
/// for every utterance, and the settings dialog switches it, so the next
/// thing spoken uses the theme chosen. Cheap to clone.
#[derive(Clone, Debug, Default)]
pub struct ThemeHandle {
    active: Arc<RwLock<Arc<ActiveTheme>>>,
}

impl ThemeHandle {
    /// A handle holding `theme`.
    #[must_use]
    pub fn new(theme: ActiveTheme) -> Self {
        Self {
            active: Arc::new(RwLock::new(Arc::new(theme))),
        }
    }

    /// The active theme.
    #[must_use]
    pub fn get(&self) -> Arc<ActiveTheme> {
        Arc::clone(&self.active.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Makes `theme` the active theme.
    pub fn set(&self, theme: ActiveTheme) {
        *self.active.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(theme);
    }

    /// Changes the settings that go with the active theme.
    pub fn set_options(&self, options: ThemeOptions) {
        let mut active = self.active.write().unwrap_or_else(PoisonError::into_inner);
        *active = Arc::new(active.with_options(options));
    }
}

/// The presenter: each span resolved through the active theme.
///
/// Content (literal text, labels, values, characters, and messages) is
/// spoken as its localized words; roles and states resolve through
/// `verbatim-i18n`; states that are never announced contribute nothing; a
/// position within a set becomes the localized "2 of 5" (and contributes
/// nothing without a set size); a level becomes the localized "level 3". The
/// words are joined with single spaces. Every span that is an indication is
/// reported as the theme says: off, its words, a sound item where it
/// stands followed by its words, or the sound alone. A capital letter spoken
/// on its own is raised in pitch by `CAPITAL_PITCH_OFFSET` (30) when the
/// capital indication is spoken, as NVDA raises it by default, and preceded
/// by its sound when it plays one. A character spoken on its own is spoken
/// by its name from the character table of its segment's language
/// ("comma"), or, with no name, as itself; its description replaces it
/// where one is asked for and the table has one. An index mark becomes a
/// mark item where it stands.
#[derive(Clone, Debug, Default)]
pub struct ThemePresenter {
    themes: ThemeHandle,
}

impl ThemePresenter {
    /// A presenter of whatever theme `themes` holds.
    #[must_use]
    pub fn new(themes: ThemeHandle) -> Self {
        Self { themes }
    }
}

impl Presenter for ThemePresenter {
    /// The sequence is text items, or none when nothing is spoken; a sound
    /// item before the words of an indication that plays one; a raised
    /// capital is a text item between two pitch changes, the second back to
    /// the configured pitch, as is the words of an indication spoken in a
    /// voice style that changes the pitch; an index mark is a mark item.
    /// The language tag is taken from the first segment that overrides it,
    /// if any.
    fn flatten(&self, utterance: &Utterance, id: UtteranceId) -> SpeechSequence {
        let active = self.themes.get();
        let mut out = Output::default();
        for segment in &utterance.segments {
            let language = segment.language.as_deref();
            if let Some(capital) = capital_of(&segment.content, language) {
                out.capital(&active, capital);
            } else if let SegmentContent::Mark(mark) = &segment.content {
                out.flush();
                out.items.push(SpeechItem::Mark(IndexMark(mark.0)));
            } else if let Some(indication) = Indication::of_segment(&segment.content) {
                out.indication(&active, indication, &segment.content, language);
            } else if let Some(part) = spoken_form(&segment.content, language) {
                out.parts.push(part);
            }
        }
        out.flush();

        let language = utterance
            .segments
            .iter()
            .find_map(|segment| segment.language.clone());

        SpeechSequence {
            utterance: id,
            trace_id: utterance.trace_id,
            language,
            items: out.items,
        }
    }
}

/// The sequence being built: items, and words not yet made a text item.
#[derive(Default)]
struct Output {
    items: Vec<SpeechItem>,
    parts: Vec<String>,
}

impl Output {
    /// Ends a run of spoken groups as one text item.
    fn flush(&mut self) {
        let text = self.parts.join(" ");
        self.parts.clear();
        if !text.is_empty() {
            self.items.push(SpeechItem::Text(text));
        }
    }

    /// Places a sound item here.
    fn sound(&mut self, indication: Indication, sound: &Arc<Sound>, gain: f32) {
        self.flush();
        self.items.push(SpeechItem::Sound(SoundCue {
            indication: indication.id(),
            sound: Arc::clone(sound),
            gain,
        }));
    }

    /// Speaks `text` with the pitch changed by `pitch`, or as it is for 0.
    fn pitched(&mut self, pitch: i32, text: String) {
        if pitch == 0 {
            self.parts.push(text);
        } else {
            self.flush();
            self.items.push(SpeechItem::Pitch(pitch));
            self.items.push(SpeechItem::Text(text));
            self.items.push(SpeechItem::Pitch(0));
        }
    }

    /// Reports a span that is an indication, as the theme says. A span
    /// with nothing to say (a state never announced, a position without a
    /// set size) says nothing and plays nothing.
    fn indication(
        &mut self,
        active: &ActiveTheme,
        indication: Indication,
        content: &SegmentContent,
        language: Option<&str>,
    ) {
        let Some(own) = spoken_form(content, language) else {
            return;
        };
        let decision = active.decide(indication);
        // An error's sound marks where it starts, not where it ends.
        let ending = matches!(
            content,
            SegmentContent::Format(TextFormat::NotSpellingError | TextFormat::NotGrammarError)
        );
        if let Some((_, sound)) = decision.sound
            && !ending
        {
            self.sound(indication, sound, decision.gain);
        }
        if decision.speak
            && let Some(words) = words_with(
                decision.setting.words.as_deref(),
                Some(own),
                carries_content(indication),
            )
        {
            self.pitched(active.voice_pitch(decision.setting), words);
        }
    }

    /// Speaks a capital letter spoken on its own, as the capital indication
    /// says: raised in pitch when it is spoken (after its replacement words,
    /// if any), preceded by its sound when it plays one, and as it is
    /// otherwise. The letter itself is content, so it is always spoken.
    fn capital(&mut self, active: &ActiveTheme, letter: String) {
        let decision = active.decide(Indication::Capital);
        if let Some((_, sound)) = decision.sound {
            self.sound(Indication::Capital, sound, decision.gain);
        }
        if decision.speak {
            if let Some(words) = &decision.setting.words {
                self.parts.push(words.clone());
            }
            self.flush();
            self.items.push(SpeechItem::Pitch(CAPITAL_PITCH_OFFSET));
            self.items.push(SpeechItem::Text(letter));
            self.items.push(SpeechItem::Pitch(0));
        } else {
            self.parts.push(letter);
        }
    }
}

/// The words for an indication: its own, or the replacement words, which
/// replace its own wording, or come before it when the indication carries
/// content of its own (`with_content`).
fn words_with(
    replacement: Option<&str>,
    own: Option<String>,
    with_content: bool,
) -> Option<String> {
    match (replacement, own) {
        (None, own) => own,
        (Some(words), Some(own)) if with_content => Some(format!("{words} {own}")),
        (Some(words), _) => Some(words.to_owned()),
    }
}

/// Whether an indication's words carry content of its own (a description,
/// a position, a font), which replacement words come before rather than
/// replace.
fn carries_content(indication: Indication) -> bool {
    matches!(
        indication,
        Indication::Description
            | Indication::Shortcut
            | Indication::Position
            | Indication::Level
            | Indication::FontName
            | Indication::FontSize
            | Indication::Color
            | Indication::SkippedLines
            | Indication::Progress
    )
}

/// The capital letter a span speaks on its own, if it is one: a spelled
/// capital, a capital character with no name of its own, or a capital's
/// description (the description is what is spoken).
fn capital_of(content: &SegmentContent, language: Option<&str>) -> Option<String> {
    match content {
        SegmentContent::SpelledCapital(text) => Some(text.clone()),
        SegmentContent::Character(text) if character_name(text, language).is_none() => {
            is_capital(text).then(|| text.clone())
        }
        SegmentContent::CharacterDescription(text) if is_capital(text) => {
            Some(character_description(text, language).unwrap_or_else(|| text.clone()))
        }
        _ => None,
    }
}

/// Whether `text` is one uppercase letter, which is raised in pitch when it
/// is spoken on its own.
fn is_capital(text: &str) -> bool {
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|first| first.is_uppercase() && chars.next().is_none())
}

/// The plain spoken form of one span, or `None` for spans with nothing to
/// say: empty text, states that are never announced, a bare position
/// without a set size (which has no useful spoken form), and any future
/// variant until it is given a spoken form here (`SegmentContent` is
/// non-exhaustive). `language` is the segment's language, for the
/// character table.
fn spoken_form(content: &SegmentContent, language: Option<&str>) -> Option<String> {
    match content {
        SegmentContent::Text(text)
        | SegmentContent::Label(text)
        | SegmentContent::Value(text)
        | SegmentContent::Description(text)
        | SegmentContent::Shortcut(text) => (!text.is_empty()).then(|| text.clone()),
        SegmentContent::Role(role) => Some(role_name(*role)),
        SegmentContent::State(state) => state_name(*state),
        SegmentContent::NegatedState(state) => negated_state_name(*state),
        SegmentContent::Position { position, set_size } => {
            set_size.map(|set_size| position_in_set(*position, set_size))
        }
        SegmentContent::Level(depth) => Some(level(*depth)),
        SegmentContent::Message(message) => {
            let text = message_text(*message);
            (!text.is_empty()).then_some(text)
        }
        SegmentContent::Phrase(phrase) => {
            let text = phrase_text(phrase);
            (!text.is_empty()).then_some(text)
        }
        SegmentContent::Format(format) => {
            let text = format_text(format);
            (!text.is_empty()).then_some(text)
        }
        SegmentContent::Character(text) => {
            character_name(text, language).or_else(|| (!text.is_empty()).then(|| text.clone()))
        }
        SegmentContent::CharacterDescription(text) => character_description(text, language)
            .or_else(|| character_name(text, language))
            .or_else(|| (!text.is_empty()).then(|| text.clone())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use verbatim_audio::PcmFormat;
    use verbatim_model::{
        Message, Role, SpeechPriority, State, Tone, TraceId, UtteranceSegment, VoiceStyle,
    };

    use super::*;

    fn utterance_of(segments: Vec<UtteranceSegment>) -> Utterance {
        Utterance {
            trace_id: TraceId::mint(),
            priority: SpeechPriority::Queued,
            segments,
            source: None,
            validity: None,
        }
    }

    fn plain() -> ThemePresenter {
        ThemePresenter::default()
    }

    /// A presenter of `theme`, with every sound file it names present as a
    /// short WAV.
    fn presenter_of(theme: Theme, options: ThemeOptions) -> ThemePresenter {
        // A file of its own for each presenter, as tests run in parallel.
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join("verbatim-speech-theme-tests");
        std::fs::create_dir_all(&dir).expect("create the sounds folder");
        let number = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let wav = dir.join(format!("short-{}-{number}.wav", std::process::id()));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&wav, spec).expect("create the WAV");
        writer.write_sample(1_000_i16).expect("write a sample");
        writer.finalize().expect("finish the WAV");
        let active = ActiveTheme::new(
            theme,
            |file| (file != "absent.wav").then(|| wav.clone()),
            options,
        );
        ThemePresenter::new(ThemeHandle::new(active))
    }

    fn setting(report: Presentation, sound: Option<&str>) -> IndicationSetting {
        IndicationSetting {
            report,
            sound: sound.map(|file| SoundSource::File(file.to_owned())),
            ..IndicationSetting::default()
        }
    }

    /// The items of a sequence with each sound as its indication's id.
    fn shape(sequence: &SpeechSequence) -> Vec<String> {
        sequence
            .items
            .iter()
            .map(|item| match item {
                SpeechItem::Text(text) => text.clone(),
                SpeechItem::Sound(cue) => format!("sound: {}", cue.indication),
                SpeechItem::Mark(mark) => format!("mark {}", mark.0),
                SpeechItem::Pitch(pitch) => format!("pitch {pitch}"),
            })
            .collect()
    }

    #[test]
    fn a_spelled_capital_is_raised_in_pitch_and_the_pitch_restored() {
        let utterance = utterance_of(vec![
            UtteranceSegment::text("a"),
            UtteranceSegment::new(SegmentContent::SpelledCapital("B".to_owned())),
            UtteranceSegment::text("c"),
        ]);
        let sequence = plain().flatten(&utterance, UtteranceId(1));
        assert_eq!(
            sequence.items,
            vec![
                SpeechItem::Text("a".to_owned()),
                SpeechItem::Pitch(CAPITAL_PITCH_OFFSET),
                SpeechItem::Text("B".to_owned()),
                SpeechItem::Pitch(0),
                SpeechItem::Text("c".to_owned()),
            ]
        );
        assert_eq!(sequence.text(), "a B c");
    }

    #[test]
    fn characters_are_spoken_by_name_description_or_raised_pitch() {
        use verbatim_model::SpeechMark;
        let utterance = utterance_of(vec![
            UtteranceSegment::new(SegmentContent::Character(",".to_owned())),
            UtteranceSegment::new(SegmentContent::Character("x".to_owned())),
            UtteranceSegment::new(SegmentContent::Mark(SpeechMark(4))),
            UtteranceSegment::new(SegmentContent::CharacterDescription("b".to_owned())),
            UtteranceSegment::new(SegmentContent::Character("Q".to_owned())),
        ]);
        let sequence = plain().flatten(&utterance, UtteranceId(1));
        assert_eq!(
            sequence.items,
            vec![
                SpeechItem::Text("comma x".to_owned()),
                SpeechItem::Mark(IndexMark(4)),
                SpeechItem::Text("Bravo".to_owned()),
                SpeechItem::Pitch(CAPITAL_PITCH_OFFSET),
                SpeechItem::Text("Q".to_owned()),
                SpeechItem::Pitch(0),
            ]
        );
        assert!(sequence.has_marks());
    }

    #[test]
    fn renders_text_and_role_joined_with_spaces() {
        let utterance = utterance_of(vec![
            UtteranceSegment::text("Settings"),
            UtteranceSegment::new(SegmentContent::Role(Role::MenuItem)),
        ]);
        let sequence = plain().flatten(&utterance, UtteranceId(1));
        assert_eq!(sequence.text(), "Settings menu item");
        assert!(!sequence.has_marks());
    }

    #[test]
    fn labels_values_and_descriptions_render_as_their_text() {
        let utterance = utterance_of(vec![
            UtteranceSegment::label("Rate"),
            UtteranceSegment::new(SegmentContent::Role(Role::Slider)),
            UtteranceSegment::value("50"),
            UtteranceSegment::new(SegmentContent::Description("Speech rate".to_owned())),
        ]);
        let sequence = plain().flatten(&utterance, UtteranceId(1));
        assert_eq!(sequence.text(), "Rate slider 50 Speech rate");
    }

    #[test]
    fn drops_silent_states_and_keeps_announced_ones() {
        let utterance = utterance_of(vec![
            UtteranceSegment::text("Bold"),
            // Focusable is never announced and must contribute nothing.
            UtteranceSegment::new(SegmentContent::State(State::Focusable)),
            UtteranceSegment::new(SegmentContent::NegatedState(State::Checked)),
        ]);
        let sequence = plain().flatten(&utterance, UtteranceId(1));
        assert_eq!(sequence.text(), "Bold not checked");
    }

    #[test]
    fn position_renders_with_a_set_size_and_not_without() {
        let with_size = utterance_of(vec![UtteranceSegment::new(SegmentContent::Position {
            position: 2,
            set_size: Some(5),
        })]);
        assert_eq!(plain().flatten(&with_size, UtteranceId(1)).text(), "2 of 5");

        let without_size = utterance_of(vec![UtteranceSegment::new(SegmentContent::Position {
            position: 2,
            set_size: None,
        })]);
        assert_eq!(plain().flatten(&without_size, UtteranceId(1)).text(), "");
    }

    #[test]
    fn level_renders_its_localized_phrase() {
        let utterance = utterance_of(vec![UtteranceSegment::new(SegmentContent::Level(3))]);
        assert_eq!(
            plain().flatten(&utterance, UtteranceId(1)).text(),
            "level 3"
        );
    }

    #[test]
    fn language_override_flows_from_first_tagged_segment() {
        let mut segment = UtteranceSegment::text("hola");
        segment.language = Some("es".to_owned());
        let utterance = utterance_of(vec![segment]);
        let sequence = plain().flatten(&utterance, UtteranceId(1));
        assert_eq!(sequence.language.as_deref(), Some("es"));
    }

    /// A misspelled word read as the default theme reads it: the sound where
    /// the error starts, then its words, and only words where it ends.
    fn misspelled() -> Utterance {
        utterance_of(vec![
            UtteranceSegment::text("the"),
            UtteranceSegment::new(SegmentContent::Format(TextFormat::SpellingError)),
            UtteranceSegment::text("wrold"),
            UtteranceSegment::new(SegmentContent::Format(TextFormat::NotSpellingError)),
            UtteranceSegment::text("turns"),
        ])
    }

    #[test]
    fn the_default_theme_plays_the_spelling_sound_at_its_place() {
        let presenter = presenter_of(Theme::builtin_default(), ThemeOptions::default());
        let sequence = presenter.flatten(&misspelled(), UtteranceId(1));
        assert_eq!(
            shape(&sequence),
            [
                "the",
                "sound: spelling-error",
                "spelling error wrold out of spelling error turns"
            ]
        );
        assert_eq!(
            sequence.text(),
            "the sound: spelling-error spelling error wrold out of spelling error turns"
        );
    }

    #[test]
    fn a_missing_sound_is_spoken_instead() {
        let mut theme = Theme::new("quiet", "Quiet");
        theme.indications.insert(
            Indication::SpellingError,
            setting(Presentation::Sound, Some("absent.wav")),
        );
        let presenter = presenter_of(theme, ThemeOptions::default());
        assert_eq!(
            presenter.themes.get().problems(),
            [ThemeProblem::MissingSound {
                indication: Indication::SpellingError,
                file: "absent.wav".to_owned()
            }]
        );
        let sequence = presenter.flatten(&misspelled(), UtteranceId(1));
        assert_eq!(
            shape(&sequence),
            ["the spelling error wrold out of spelling error turns"]
        );
    }

    #[test]
    fn sound_alone_off_and_speech_alone_are_each_honoured() {
        let mut theme = Theme::new("mixed", "Mixed");
        theme.indications.insert(
            Indication::Role(Role::Link),
            setting(Presentation::Sound, Some("link.wav")),
        );
        theme
            .indications
            .insert(Indication::Description, setting(Presentation::Off, None));
        theme.indications.insert(
            Indication::SpellingError,
            setting(Presentation::Speech, Some("textError.wav")),
        );
        let presenter = presenter_of(theme.clone(), ThemeOptions::default());
        let utterance = utterance_of(vec![
            UtteranceSegment::label("Home"),
            UtteranceSegment::new(SegmentContent::Role(Role::Link)),
            UtteranceSegment::new(SegmentContent::Description("Go home".to_owned())),
            UtteranceSegment::new(SegmentContent::Format(TextFormat::SpellingError)),
        ]);
        assert_eq!(
            shape(&presenter.flatten(&utterance, UtteranceId(1))),
            ["Home", "sound: role-link", "spelling error"]
        );

        // Learning a theme: sounded indications are spoken as well.
        let learning = presenter_of(
            theme,
            ThemeOptions {
                speak_sounded_indications: true,
                ..ThemeOptions::default()
            },
        );
        assert_eq!(
            shape(&learning.flatten(&utterance, UtteranceId(1))),
            ["Home", "sound: role-link", "link spelling error"]
        );
    }

    #[test]
    fn words_replace_or_precede_and_a_voice_style_changes_the_pitch() {
        let mut theme = Theme::new("worded", "Worded");
        theme.voice_styles.insert(
            "low".to_owned(),
            VoiceStyle {
                pitch: -20,
                ..VoiceStyle::default()
            },
        );
        theme.indications.insert(
            Indication::Role(Role::Button),
            IndicationSetting {
                words: Some("btn".to_owned()),
                voice: Some("low".to_owned()),
                ..IndicationSetting::default()
            },
        );
        theme.indications.insert(
            Indication::Position,
            IndicationSetting {
                words: Some("item".to_owned()),
                ..IndicationSetting::default()
            },
        );
        let presenter = presenter_of(theme, ThemeOptions::default());
        let utterance = utterance_of(vec![
            UtteranceSegment::label("OK"),
            UtteranceSegment::new(SegmentContent::Role(Role::Button)),
            UtteranceSegment::new(SegmentContent::Position {
                position: 1,
                set_size: Some(2),
            }),
        ]);
        assert_eq!(
            shape(&presenter.flatten(&utterance, UtteranceId(1))),
            ["OK", "pitch -20", "btn", "pitch 0", "item 1 of 2"]
        );
    }

    #[test]
    fn a_capital_can_beep_instead_of_rising() {
        let mut theme = Theme::new("beeps", "Beeps");
        theme.indications.insert(
            Indication::Capital,
            IndicationSetting {
                report: Presentation::Sound,
                sound: Some(SoundSource::Tone(Tone {
                    frequency_hz: 1_760,
                    duration_ms: 40,
                })),
                ..IndicationSetting::default()
            },
        );
        let presenter = presenter_of(theme, ThemeOptions::default());
        let utterance = utterance_of(vec![UtteranceSegment::new(SegmentContent::SpelledCapital(
            "B".to_owned(),
        ))]);
        assert_eq!(
            shape(&presenter.flatten(&utterance, UtteranceId(1))),
            ["sound: capital", "B"]
        );
    }

    #[test]
    fn gains_multiply_and_switching_themes_applies_at_once() {
        let mut theme = Theme::new("loud", "Loud");
        theme.gain = 200;
        theme.indications.insert(
            Indication::Blank,
            IndicationSetting {
                report: Presentation::Sound,
                sound: Some(SoundSource::File("blank.wav".to_owned())),
                gain: 50,
                ..IndicationSetting::default()
            },
        );
        let presenter = presenter_of(
            theme,
            ThemeOptions {
                sound_volume: 40,
                ..ThemeOptions::default()
            },
        );
        let blank = utterance_of(vec![UtteranceSegment::new(SegmentContent::Message(
            Message::Blank,
        ))]);
        let sequence = presenter.flatten(&blank, UtteranceId(1));
        let Some(SpeechItem::Sound(cue)) = sequence.items.first() else {
            panic!("a sound: {sequence:?}");
        };
        assert!((cue.gain - 0.4).abs() < 1e-6, "{}", cue.gain);
        assert_eq!(
            cue.sound.format(),
            PcmFormat {
                sample_rate: 8_000,
                channels: 1
            }
        );

        presenter.themes.set(ActiveTheme::default());
        assert_eq!(presenter.flatten(&blank, UtteranceId(2)).text(), "blank");
    }

    #[test]
    fn an_event_is_reported_by_its_sound_or_its_words() {
        let active = presenter_of(Theme::builtin_default(), ThemeOptions::default())
            .themes
            .get();
        let (sound, words) = active.earcon(Earcon::BrowseMode);
        assert!(sound.is_some() && words.is_none());
        let (progress, _) = active.earcon(Earcon::Progress(100));
        let (start, _) = active.earcon(Earcon::Progress(0));
        assert!(progress.is_some() && start.is_some());

        // With no sound files loaded, an event whose sound is a file is
        // spoken; tones need no files.
        let silent = ActiveTheme::default();
        let (sound, words) = silent.earcon(Earcon::BrowseMode);
        assert!(sound.is_none());
        assert_eq!(words.as_deref(), Some("browse mode"));
        assert!(silent.earcon(Earcon::Progress(40)).0.is_some());

        let mut spoken = Theme::new("spoken", "Spoken");
        spoken
            .indications
            .insert(Indication::Progress, setting(Presentation::Speech, None));
        let spoken = ActiveTheme::new(spoken, |_| None, ThemeOptions::default());
        let (sound, words) = spoken.earcon(Earcon::Progress(40));
        assert!(sound.is_none());
        assert_eq!(words.as_deref(), Some("40 percent"));
    }

    #[test]
    fn the_shipped_sounds_all_decode() {
        let sounds = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sounds");
        let active = ActiveTheme::new(
            Theme::builtin_default(),
            |file| Some(sounds.join(file)).filter(|path| path.is_file()),
            ThemeOptions::default(),
        );
        assert_eq!(active.problems(), []);
    }
}
