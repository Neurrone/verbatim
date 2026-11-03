//! The pure decision state machine.
//!
//! Every rule about what a keystroke means to Verbatim lives here, and
//! nothing here touches the operating system: the machine turns a stream of
//! [`KeyEvent`]s into swallow-or-pass [`Decision`]s and emitted gestures,
//! driven only by its configuration, the gesture map it owns, and the time
//! the caller gives with each key. The hook thread (in `verbatim-input-windows`) is the thin
//! imperative shell that feeds it real events, which is what lets the whole
//! of Verbatim's keyboard behaviour be exercised by ordinary unit tests with
//! scripted key streams and scripted time.
//!
//! `docs/parity/input.md` is the behavioural record: which of
//! these rules exist for parity with other screen readers, and which
//! behaviours Verbatim has decided not to have. [`DecisionMachine`] carries
//! the summary of both.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::Duration;

use verbatim_model::{GestureId, TraceId};

use crate::keys;
use crate::map::GestureMap;
use crate::{KeyEvent, ToggleKey};

/// Virtual-key code of caps lock.
const VK_CAPITAL: u16 = 0x14;
/// Virtual-key code shared by the extended insert and the numpad insert; the
/// extended flag distinguishes them.
const VK_INSERT: u16 = 0x2D;

/// The default multi-press timeout, matching NVDA's default (500 ms).
pub const DEFAULT_MULTI_PRESS_TIMEOUT: Duration = Duration::from_millis(500);

/// Which physical keys act as the Verbatim modifier, plus the multi-press
/// timeout that governs the double-tap passthrough.
///
/// The three booleans mirror [`verbatim_config::VerbatimKeys`] but are
/// redeclared here so this crate stays decoupled from configuration; the
/// application maps one to the other. All three default to enabled, matching
/// the configuration crate's defaults.
///
/// [`verbatim_config::VerbatimKeys`]: https://docs.rs/verbatim-config
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent per-key toggles plus one behavior flag, not a state machine"
)]
pub struct DecisionConfig {
    /// Caps lock acts as the Verbatim modifier.
    pub caps_lock: bool,
    /// The extended (navigation-cluster) insert key acts as the Verbatim
    /// modifier.
    pub insert: bool,
    /// The numpad insert key acts as the Verbatim modifier.
    pub numpad_insert: bool,
    /// Passes the Verbatim modifier's own transitions down the hook chain
    /// instead of swallowing them, so another screen reader hooked behind
    /// Verbatim sees the modifier held and can run its own commands for
    /// chords Verbatim leaves unbound. The modifier still works as
    /// Verbatim's modifier internally. Only sound while such a screen
    /// reader is running: with nothing behind Verbatim to swallow the key,
    /// caps lock reaches the OS and toggles on every use.
    pub share_modifier: bool,
    /// How quickly a lone Verbatim-modifier tap must be repeated to pass the
    /// key through to the operating system (double-tap passthrough).
    pub multi_press_timeout: Duration,
    /// NVDA's "Speech interrupt for typed characters": when false, a typed
    /// character and Shift leave speech alone.
    pub interrupt_for_characters: bool,
    /// NVDA's "Speech interrupt for Enter key": when false, Enter leaves
    /// speech alone.
    pub interrupt_for_enter: bool,
}

impl Default for DecisionConfig {
    fn default() -> Self {
        Self {
            caps_lock: true,
            insert: true,
            numpad_insert: true,
            share_modifier: false,
            multi_press_timeout: DEFAULT_MULTI_PRESS_TIMEOUT,
            interrupt_for_characters: true,
            interrupt_for_enter: true,
        }
    }
}

/// A gesture the state machine decided to raise: the hook stamps it with a
/// trace id ([`RaisedGesture::stamped`]) as it reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RaisedGesture {
    /// The bound gesture that fired.
    pub gesture: GestureId,
    /// The multi-press repeat count: 0 for the first press, 1 for the
    /// second, 2 for the third, and so on, saturating rather than
    /// overflowing. Incremented when the same gesture fires again within
    /// `multi_press_timeout` of its previous genuine press; a different
    /// gesture or the window elapsing resets it to 0. Consumers dispatch on
    /// it for report-current (report, spell, copy), speak time (time, then
    /// date), and show tray list (tray, then taskbar). Auto-repeat (holding
    /// the gesture's key down, which re-fires the gesture on every OS
    /// auto-repeat tick with no intervening key-up) does not advance this
    /// count: a held key is one press for repeat purposes, so every
    /// auto-repeated emission carries the same count as the genuine press
    /// that started the hold. NVDA counts auto-repeat as presses; this is a
    /// deliberate difference (`docs/parity/input.md`, "Script repeat counting").
    /// Any key other than a modifier that completes no bound gesture ends
    /// the streak, as in NVDA.
    pub repeat: u8,
}

impl RaisedGesture {
    /// This gesture, tagged with `trace_id` for tracing.
    #[must_use]
    pub fn stamped(self, trace_id: TraceId) -> EmittedGesture {
        EmittedGesture {
            trace_id,
            gesture: self.gesture,
            repeat: self.repeat,
        }
    }
}

/// A raised gesture tagged for tracing, as the hook reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmittedGesture {
    /// A freshly minted trace ID correlating this keypress with everything it
    /// causes downstream (architecture section 9).
    pub trace_id: TraceId,
    /// The gesture that fired.
    pub gesture: GestureId,
    /// Its multi-press repeat count ([`RaisedGesture::repeat`]).
    pub repeat: u8,
}

/// What a key press does to speech (`docs/nvda/input.md`, "What a key
/// press does to speech").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySpeechEffect {
    /// Cancel current and queued speech.
    Cancel,
    /// Pause speech, or resume it when paused: Shift on its own.
    TogglePause,
}

/// Who made a key transition, as the hook decodes it from the key's
/// extra information.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySource {
    /// The user, or the end-to-end harness typing on the user's behalf.
    User,
    /// Verbatim itself, for its own purposes (the Control tap that lets it
    /// take the foreground): never held nor deferred, and reported only as
    /// a bound gesture or a lock key.
    Own,
}

/// What Core's view of the window in front lets the machine do with a key
/// that would reach the application (`docs/design/key-holding.md`). One
/// value, so that a focus with text in a window that may not hold keys
/// cannot be given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HoldPermit {
    /// Neither hold nor defer: Core's view is not about the window in
    /// front, or does not let keys be held there.
    None,
    /// Defer a key behind a held one, but hold none: the focus has no text.
    Defer,
    /// Hold a caret key, and defer behind it: the focus has text.
    HoldAndDefer,
}

/// One key transition with everything the machine decides it by, gathered
/// by the hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyInput {
    /// The transition.
    pub event: KeyEvent,
    /// When it was observed, in microseconds on the run's shared clock.
    pub now: u64,
    /// Whether Num Lock is on, the keyboard's own state, read by the hook
    /// before the key. With it on, the numpad's operator keys (plus, minus,
    /// multiply, divide) type their characters and complete no gesture, as
    /// NVDA treats Num Lock as a modifier of them, so a binding of numpad
    /// plus does not take the plus sign from a user typing numbers.
    pub num_lock: bool,
    /// Who made it.
    pub source: KeySource,
    /// What Core's view lets the machine do with it.
    pub permit: HoldPermit,
}

/// What the hook does with a key transition: pass it, or swallow it, for
/// good or to be sent again from its place in the line of keys held back
/// (`docs/design/key-holding.md`, "Keys behind a held key").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Pass it to the application now.
    Pass,
    /// Swallow it for good.
    Swallow,
    /// A caret key's press, held back for its caret to be read, with its
    /// place in line: Core has it sent again, press and release, once read.
    Hold(u32),
    /// A transition that would have passed, deferred behind a held key,
    /// with its place in line: Core has it sent again in its turn.
    Defer(u32),
}

/// What a press that reaches the application, now or once deferred,
/// reports of what it did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PressReport {
    /// Nothing: a release, a key swallowed or held, the Verbatim modifier
    /// shared, or Verbatim's own key.
    Nothing,
    /// The text it types, which the hook translates.
    Translated,
    /// The text it types, or, typing none, that it may have edited the
    /// text: it completed no observed gesture and is no modifier, lock key
    /// or key Windows gives a meaning outside the text.
    TranslatedOrPassed,
}

/// The outcome of feeding one [`KeyInput`] to the state machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    /// What the hook does with the transition.
    pub verdict: Verdict,
    /// The gesture this transition raised, if any. Only ever set on a
    /// [`Verdict::Swallow`].
    pub emitted: Option<RaisedGesture>,
    /// What this transition does to speech, carried out before the gesture
    /// is acted on. Set on nearly every key-down, whether it is bound,
    /// swallowed, or passed on; never for Verbatim's own key.
    pub speech: Option<KeySpeechEffect>,
    /// Whether a passed transition is the Verbatim modifier key's own, held
    /// or double-tapped, passed in share mode for the screen reader hooked
    /// behind Verbatim
    /// ([`DecisionConfig::share_modifier`]). That reader decides whether the
    /// key reaches the operating system, so whether a lock key used this
    /// way changes state is not known here.
    pub shared_modifier: bool,
    /// An observed gesture this key-down completed ([`GestureMap::observes`]):
    /// the key is passed to the application, and the gesture reported, so
    /// the reducer can speak what the application did with it (a caret
    /// key). Never set on a [`Verdict::Swallow`], nor for Verbatim's own
    /// key; its `repeat` is always 0.
    pub observed: Option<RaisedGesture>,
    /// What a press that reaches the application reports.
    pub typing: PressReport,
    /// The lock key whose new state is announced: one pressed and passed
    /// to the operating system, not as the Verbatim modifier shared.
    pub toggle: Option<ToggleKey>,
}

impl Decision {
    /// Pass the transition through, raising nothing.
    fn pass() -> Self {
        Self {
            verdict: Verdict::Pass,
            emitted: None,
            speech: None,
            shared_modifier: false,
            observed: None,
            typing: PressReport::Nothing,
            toggle: None,
        }
    }

    /// Pass the transition through, reporting the observed gesture.
    fn pass_observed(gesture: GestureId) -> Self {
        Self {
            observed: Some(RaisedGesture { gesture, repeat: 0 }),
            ..Self::pass()
        }
    }

    /// Pass the Verbatim modifier's own transition in share mode.
    fn pass_shared_modifier() -> Self {
        Self {
            shared_modifier: true,
            ..Self::pass()
        }
    }

    /// Swallow the transition, raising nothing.
    fn swallow() -> Self {
        Self {
            verdict: Verdict::Swallow,
            ..Self::pass()
        }
    }

    /// Swallow the transition and raise a gesture.
    fn swallow_emit(emitted: RaisedGesture) -> Self {
        Self {
            verdict: Verdict::Swallow,
            emitted: Some(emitted),
            ..Self::pass()
        }
    }
}

/// Virtual-key codes the speech effect treats specially.
const VK_RETURN: u16 = 0x0D;
const VK_SPACE: u16 = 0x20;
const VK_NUMLOCK: u16 = 0x90;
const VK_SCROLL: u16 = 0x91;
const VK_SHIFT: u16 = 0x10;
const VK_LSHIFT: u16 = 0xA0;
const VK_RSHIFT: u16 = 0xA1;
const VK_VOLUME_MUTE: u16 = 0xAD;
const VK_VOLUME_UP: u16 = 0xAF;
/// A key Windows does not know, which some devices report for events such
/// as a gyroscope moving.
const VK_UNKNOWN: u16 = 0xFF;

/// Identity of a physical key: the virtual-key code paired with the extended
/// flag, so an extended key and its numpad twin are distinct keys.
type KeyCode = (u16, bool);

/// What the machine knows of a key it has seen go down and not come up.
/// All four combinations are valid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct KeyDown {
    /// A press of the key has reached the application, or is deferred to
    /// reach it, since it last went down or since its last held press,
    /// whose release Core sends with it. Its release is swallowed while
    /// this is false, so the application never sees a release without its
    /// press, and delivered once it is true, so a key one of whose presses
    /// reached the application is seen to come up.
    reached: bool,
    /// A press of the key completed a bound gesture, or was the Verbatim
    /// modifier's, and was swallowed, since it last went down: a later
    /// press is the operating system's auto-repeat, for multi-press
    /// counting.
    swallowed_gesture: bool,
}

/// A key transition held back or deferred, not yet passed on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Waiting {
    /// Its place in line.
    place: u32,
    /// A deferred transition's virtual-key code and whether it is a press;
    /// `None` for a held caret key, whose release Core sends with it.
    transition: Option<(u16, bool)>,
}

/// The part a key plays in the press currently being decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyRole {
    /// The key is acting as the Verbatim modifier right now. It names itself
    /// `verbatim` in a chord, and Verbatim owns its transitions.
    Modifier,
    /// An ordinary key Verbatim can name, and so can place in a chord.
    Named(&'static str),
    /// A key Verbatim has no name for. It can take no part in a gesture, so
    /// it is the application's business alone.
    Unnamed,
}

/// Where a lone press of a Verbatim-modifier key stands in its lifecycle.
///
/// Tapping a modifier key on its own is how the user asks for that key's
/// ordinary meaning back: tap it once, then again before
/// [`DecisionConfig::multi_press_timeout`] elapses, and the second press goes
/// to the operating system, so caps lock toggles and insert inserts. The
/// facts that lifecycle needs — which key is the candidate, when it was let
/// go, and whether a press is being handed over — tell one story, so they are
/// one value. The combinations this enum cannot express are exactly the ones
/// that would be nonsense: a release time belonging to no key, or a hand-over
/// with no tap behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LoneModifier {
    /// No tap is in flight. Either no modifier key is down, or another key
    /// has gone down since one was, which disqualifies the hold: a modifier
    /// used in a chord is not a lone tap.
    Idle,
    /// A Verbatim-modifier key is down and nothing else has gone down since,
    /// so releasing it will still count as a lone tap.
    HeldAlone(KeyCode),
    /// The lone tap completed: `key` was pressed by itself and let go at
    /// `released`. Pressing that same key again before the multi-press
    /// timeout elapses hands the press to the operating system.
    TapPending {
        /// The key that was tapped alone; only that key re-taps it.
        key: KeyCode,
        /// When the tap ended, in microseconds on the run's shared clock,
        /// which starts the multi-press window.
        released: u64,
    },
    /// A press is being handed to the operating system, and so is every
    /// transition after it until a key-up ends the hand-over. It has to
    /// outlast a single key-down because a held key auto-repeats, and it
    /// never re-arms itself, so the third tap of a triple tap is the
    /// modifier again.
    PassingThrough,
}

impl LoneModifier {
    /// Whether a press of `key` belongs to the operating system rather than
    /// to Verbatim: the second tap of a double tap, or any press while a
    /// hand-over is still running.
    fn hands_press_to_os(self, key: KeyCode, now: u64, timeout: Duration) -> bool {
        match self {
            Self::PassingThrough => true,
            Self::TapPending {
                key: tapped,
                released,
            } => tapped == key && since(released, now) < timeout,
            Self::Idle | Self::HeldAlone(_) => false,
        }
    }

    /// The lifecycle a key-down leaves behind: a hand-over outlives the press
    /// that began it, a lone modifier press becomes the candidate for the
    /// next double tap, and any other key going down spends whatever
    /// candidate there was.
    fn after_press(key: KeyCode, hands_to_os: bool, role: KeyRole) -> Self {
        if hands_to_os {
            Self::PassingThrough
        } else if role == KeyRole::Modifier {
            Self::HeldAlone(key)
        } else {
            Self::Idle
        }
    }

    /// The lifecycle a key-up leaves behind: letting the candidate go starts
    /// (or restarts) the multi-press window, any release ends a hand-over
    /// since there will be no further auto-repeats, and a release of some
    /// other key changes nothing.
    fn after_release(self, key: KeyCode, now: u64) -> Self {
        match self {
            Self::PassingThrough => Self::Idle,
            Self::HeldAlone(candidate) | Self::TapPending { key: candidate, .. }
                if candidate == key =>
            {
                Self::TapPending { key, released: now }
            }
            unchanged => unchanged,
        }
    }
}

/// The pure keyboard decision state machine.
///
/// Construct one with [`DecisionMachine::new`], then call
/// [`on_key`](DecisionMachine::on_key) for every raw key transition. The
/// machine owns its [`GestureMap`], given at construction, and reads no
/// clock, trace id or shared state: the time, Num Lock, the key's source
/// and Core's permit come with each key ([`KeyInput`]).
///
/// # The rules
///
/// - A Verbatim-modifier key-down is swallowed, so caps lock never toggles
///   while it is serving as the modifier. The exception is the double-tap
///   passthrough below.
/// - While the modifier is held, a key-down completing a bound gesture is
///   swallowed and the gesture emitted. A companion key that completes
///   nothing reaches the application as a bare keypress instead of vanishing,
///   while the modifier's own transitions stay swallowed.
/// - Tapping the modifier alone and tapping it again within
///   `multi_press_timeout` hands the second press, and everything up to its
///   release, to the operating system, so the key does what it says on the
///   keycap. That hand-over does not re-arm itself: a third tap is the
///   modifier again.
/// - A key swallowed on the way down is swallowed on the way up too, so no
///   application ever sees the release of a press it never saw; once a
///   press of it has reached the application, its release does, so no key
///   is left down.
/// - While Core's view lets keys be held ([`HoldPermit`]), a caret key's
///   press is held back for its caret to be read and sent again by Core,
///   and every later transition that would pass waits behind it in line
///   (`docs/design/key-holding.md`).
/// - Injected keys are decided exactly like physical ones, which is what lets
///   the control plane drive real gestures with synthetic input.
/// - In share mode ([`DecisionConfig::share_modifier`]) the modifier's own
///   transitions pass down the hook chain rather than being swallowed, so a
///   screen reader hooked behind Verbatim can use the same modifier for the
///   chords Verbatim leaves unbound.
/// - Multi-press counting: each [`EmittedGesture`] carries a `repeat` count.
///   Pressing the same bound gesture again within `multi_press_timeout` of
///   its previous genuine press increments the count (0, 1, 2, and on,
///   saturating); a different gesture or the window elapsing resets it to 0.
///   Holding a gesture's key down auto-repeats its key-down with no
///   intervening key-up, and every such emission carries the count of the
///   press that started the hold: auto-repeat is not a multi-press, a
///   deliberate difference from NVDA, which counts it. A key other than a
///   modifier that completes no bound gesture ends the streak, as in NVDA.
///
/// `docs/parity/input.md` records which of these match other
/// screen readers and how far that has been verified.
///
/// # Behaviours Verbatim deliberately does not have
///
/// - No latching or locking of the Verbatim modifier for Sticky Keys users;
///   that belongs with the wider accessibility-integration work.
/// - No user-invoked one-shot passthrough command, the kind that arms the
///   next keystroke to reach the application untouched. It is a feature in
///   its own right, not part of these rules.
/// - No name-guessing for virtual keys outside [`keys::name_from_vk`]'s
///   vocabulary. A key Verbatim cannot name simply passes through, rather
///   than being described by whatever the keyboard layout would call it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecisionMachine {
    config: DecisionConfig,
    map: GestureMap,
    /// Modifier keys currently held down, both normal modifiers and the
    /// Verbatim modifier, so a later key-down can form a chord with them.
    held_modifiers: BTreeSet<KeyCode>,
    /// Every key seen go down and not yet come up, with what its presses
    /// did.
    keys_down: BTreeMap<KeyCode, KeyDown>,
    /// How far a lone Verbatim-modifier tap has got towards handing its key
    /// back to the operating system.
    lone_modifier: LoneModifier,
    /// The gesture and observation time, in microseconds on the run's
    /// shared clock, of the most recent genuine (not auto-repeated)
    /// bound-gesture press, for multi-press counting.
    last_gesture: Option<(GestureId, u64)>,
    /// The multi-press repeat count of
    /// [`last_gesture`](Self::last_gesture); carried unchanged across
    /// auto-repeats of the same held gesture.
    repeat_count: u8,
    /// The key transitions held back or deferred and not yet passed on, in
    /// the order pressed; while any is, every later transition that would
    /// pass is deferred behind them.
    line: VecDeque<Waiting>,
    /// The last place in line given out.
    last_place: u32,
}

impl DecisionMachine {
    /// Creates a machine with the given configuration, deciding against the
    /// gestures of `map`.
    #[must_use]
    pub fn new(config: DecisionConfig, map: GestureMap) -> Self {
        Self {
            config,
            map,
            held_modifiers: BTreeSet::new(),
            keys_down: BTreeMap::new(),
            lone_modifier: LoneModifier::Idle,
            last_gesture: None,
            repeat_count: 0,
            line: VecDeque::new(),
            last_place: 0,
        }
    }

    /// Decides one key transition whole, and returns its verdict with what
    /// it reports.
    pub fn on_key(&mut self, input: KeyInput) -> Decision {
        let KeyInput {
            event,
            now,
            num_lock,
            source,
            permit,
        } = input;
        let mut decision = if event.pressed {
            let speech = self.speech_effect(event);
            let mut decision = self.on_press(event, now, num_lock);
            decision.speech = speech;
            decision
        } else {
            self.on_release(event, now)
        };
        decision.verdict = self.place(event, source, permit, &decision);
        report(event, source, &mut decision);
        decision
    }

    /// Notes that the key Core passed on with place `place` reached the
    /// hook on its way to the application, and with it every key ahead of
    /// it, since keys are passed on in order: one Windows dropped is not
    /// waited for once a later one arrives. A place not in line changes
    /// nothing; nothing else changes either.
    pub fn passed_on(&mut self, place: u32) {
        if !self.line.iter().any(|waiting| waiting.place == place) {
            return;
        }
        while let Some(front) = self.line.pop_front() {
            if front.place == place {
                break;
            }
        }
    }

    /// Whether a key transition waits in the line.
    #[must_use]
    pub fn has_outstanding(&self) -> bool {
        !self.line.is_empty()
    }

    /// The deferred transitions not yet passed on, in order: each key's
    /// virtual-key code and whether it is a press.
    pub fn deferred_transitions(&self) -> impl Iterator<Item = (u16, bool)> + '_ {
        self.line.iter().filter_map(|waiting| waiting.transition)
    }

    /// The verdict on a transition the rules above passed or swallowed,
    /// recording what it did to its key (`docs/crates/verbatim-input.md`): a caret key's press is held while the
    /// permit lets it be; any other transition that would pass is deferred
    /// while a transition waits in the line and the permit lets it be,
    /// unless it is a lock key or Verbatim's own; a release is swallowed
    /// while no press of its key has reached the application since it went
    /// down or was last held.
    fn place(
        &mut self,
        event: KeyEvent,
        source: KeySource,
        permit: HoldPermit,
        decision: &Decision,
    ) -> Verdict {
        let key: KeyCode = (event.vk, event.extended);
        if event.pressed {
            let held = source == KeySource::User
                && permit == HoldPermit::HoldAndDefer
                && decision
                    .observed
                    .as_ref()
                    .is_some_and(|observed| self.map.holds(&observed.gesture));
            let down = self.keys_down.entry(key).or_default();
            if decision.verdict == Verdict::Swallow {
                return Verdict::Swallow;
            }
            // A held press is sent again with its own release, so it
            // leaves nothing of this key down in the application.
            down.reached = !held;
            if held {
                return Verdict::Hold(self.queue(None));
            }
        } else if self
            .keys_down
            .remove(&key)
            .is_some_and(|down| !down.reached)
        {
            return Verdict::Swallow;
        }
        let deferred = source == KeySource::User
            && ToggleKey::from_vk(event.vk).is_none()
            && permit != HoldPermit::None
            && !self.line.is_empty();
        if deferred {
            Verdict::Defer(self.queue(Some((event.vk, event.pressed))))
        } else {
            Verdict::Pass
        }
    }

    /// Puts a transition held back or deferred in line, giving it the next
    /// place.
    fn queue(&mut self, transition: Option<(u16, bool)>) -> u32 {
        self.last_place = self.last_place.checked_add(1).unwrap_or(1);
        self.line.push_back(Waiting {
            place: self.last_place,
            transition,
        });
        self.last_place
    }

    /// What a key-down does to speech, as NVDA decides it
    /// (`docs/nvda/input.md`, "What a key press does to speech"): the
    /// volume keys and a key Windows does not know leave speech alone; with
    /// speech interrupt for typed characters off, so do a typed character
    /// and Shift; with speech interrupt for Enter off, so does Enter; Shift
    /// on its own pauses or resumes speech, but not while held, where
    /// Windows repeats it; every other key cancels it, modifiers included.
    /// Read before the press is recorded as held.
    fn speech_effect(&self, event: KeyEvent) -> Option<KeySpeechEffect> {
        let key: KeyCode = (event.vk, event.extended);
        if (event.extended && (VK_VOLUME_MUTE..=VK_VOLUME_UP).contains(&event.vk))
            || event.vk == VK_UNKNOWN
        {
            return None;
        }
        let is_shift = matches!(event.vk, VK_SHIFT | VK_LSHIFT | VK_RSHIFT);
        if !self.config.interrupt_for_characters && (is_shift || self.is_typed_character(event)) {
            return None;
        }
        if !self.config.interrupt_for_enter && event.vk == VK_RETURN {
            return None;
        }
        if is_shift {
            if self.held_modifiers.contains(&key) {
                return None;
            }
            return Some(KeySpeechEffect::TogglePause);
        }
        Some(KeySpeechEffect::Cancel)
    }

    /// Whether a key-down types a character, as NVDA's typed-character
    /// interrupt setting counts one: a key whose name is one character (a
    /// letter, a digit, a punctuation key) or Space, pressed with no
    /// modifier held or with Shift alone; a lock key counts too, unless it
    /// is serving as the Verbatim modifier.
    fn is_typed_character(&self, event: KeyEvent) -> bool {
        let vk = event.vk;
        if self.is_verbatim_modifier(vk, event.extended) {
            return false;
        }
        let modifiers_allow = self
            .held_modifiers
            .iter()
            .all(|&(held, _)| matches!(held, VK_SHIFT | VK_LSHIFT | VK_RSHIFT));
        let character_key = vk == VK_SPACE
            || (0x30..=0x39).contains(&vk)
            || (0x41..=0x5A).contains(&vk)
            || (0xBA..=0xC0).contains(&vk)
            || (0xDB..=0xDF).contains(&vk)
            || vk == 0xE2;
        let lock_key = matches!(vk, VK_CAPITAL | VK_NUMLOCK | VK_SCROLL);
        lock_key || (character_key && modifiers_allow)
    }

    /// Decides a key-down in three steps: what part the key plays, where that
    /// leaves the lone-modifier lifecycle, and what chord it completes.
    fn on_press(&mut self, event: KeyEvent, now: u64, num_lock: bool) -> Decision {
        let key: KeyCode = (event.vk, event.extended);

        // Step one: the key's part in this press. A modifier key whose press
        // belongs to the operating system is not acting as the modifier, so
        // it is named and treated like any other ordinary key.
        let hands_to_os =
            self.lone_modifier
                .hands_press_to_os(key, now, self.config.multi_press_timeout);
        let role = self.role_of(key, hands_to_os);

        // Step two: advance the lifecycle, whatever this press turns out to
        // mean, so the record of lone taps is right even for keys Verbatim
        // will decline to name.
        self.lone_modifier = LoneModifier::after_press(key, hands_to_os, role);

        // Step three: name the key, build the chord it completes, and decide.
        let main_name = match role {
            KeyRole::Modifier => keys::VERBATIM_MODIFIER_NAME,
            KeyRole::Named(_) if num_lock && is_numpad_operator(event.vk) => {
                self.last_gesture = None;
                return Decision::pass();
            }
            KeyRole::Named(name) => name,
            KeyRole::Unnamed => {
                self.last_gesture = None;
                return Decision::pass();
            }
        };
        if role == KeyRole::Modifier || is_normal_modifier(event.vk) {
            self.held_modifiers.insert(key);
        }

        let gesture = self.build_gesture(key, main_name);
        if let Some(gesture) = &gesture
            && self.map.contains(gesture)
        {
            let repeat = self.count_press(gesture, key, now);
            self.keys_down.entry(key).or_default().swallowed_gesture = true;
            return Decision::swallow_emit(RaisedGesture {
                gesture: gesture.clone(),
                repeat,
            });
        }

        // Nothing bound. A key other than a modifier ends the multi-press
        // streak, as NVDA forgets its last script when an unbound gesture
        // comes between two presses of a bound one.
        if role != KeyRole::Modifier && !is_normal_modifier(event.vk) {
            self.last_gesture = None;
        }

        // A companion key that completes no gesture reaches
        // the application as a bare keypress, so pressing the modifier and
        // page-up still pages the application instead of doing nothing at
        // all. The modifier key itself is still swallowed, since caps lock
        // must not toggle while it is the modifier; in share mode its real
        // transitions go down the hook chain instead, for the screen reader
        // hooked behind Verbatim to see and swallow in its turn.
        if role == KeyRole::Modifier {
            if self.config.share_modifier {
                return Decision::pass_shared_modifier();
            }
            self.keys_down.entry(key).or_default().swallowed_gesture = true;
            return Decision::swallow();
        }
        // A double tap passes the modifier key to the operating system; in
        // share mode the screen reader behind Verbatim decides that press's
        // fate as much as any other.
        if hands_to_os && self.config.share_modifier {
            return Decision::pass_shared_modifier();
        }
        if let Some(gesture) = gesture
            && self.map.observes(&gesture)
        {
            return Decision::pass_observed(gesture);
        }
        Decision::pass()
    }

    /// Decides a key-up: where the release leaves the lone-modifier
    /// lifecycle. Whether it is swallowed is decided by what its key's
    /// presses did ([`place`](Self::place)).
    fn on_release(&mut self, event: KeyEvent, now: u64) -> Decision {
        let key: KeyCode = (event.vk, event.extended);
        self.lone_modifier = self.lone_modifier.after_release(key, now);
        self.held_modifiers.remove(&key);
        Decision::pass()
    }

    /// What part a key plays in the press being decided. A configured
    /// modifier key acts as the modifier unless its press has been handed to
    /// the operating system, in which case it is just the key on the keycap.
    fn role_of(&self, key: KeyCode, hands_to_os: bool) -> KeyRole {
        let (vk, extended) = key;
        if !hands_to_os && self.is_verbatim_modifier(vk, extended) {
            KeyRole::Modifier
        } else if let Some(name) = keys::name_from_vk(vk, extended) {
            KeyRole::Named(name)
        } else {
            KeyRole::Unnamed
        }
    }

    /// The repeat count this emission carries, advancing the multi-press
    /// streak if the press is a genuine one.
    ///
    /// A key-down for a key Verbatim swallowed for a gesture or as the
    /// Verbatim modifier and has not yet seen released is the operating
    /// system auto-repeating a held key, not a fresh press:
    /// it re-reports the streak's current count and leaves the streak alone.
    /// NVDA counts auto-repeat as presses; not counting it is a deliberate
    /// difference, so holding a key never turns into a double press.
    fn count_press(&mut self, gesture: &GestureId, key: KeyCode, now: u64) -> u8 {
        if self
            .keys_down
            .get(&key)
            .is_some_and(|down| down.swallowed_gesture)
        {
            return self.repeat_count;
        }
        self.repeat_count = match &self.last_gesture {
            Some((last, at))
                if last == gesture && since(*at, now) < self.config.multi_press_timeout =>
            {
                self.repeat_count.saturating_add(1)
            }
            _ => 0,
        };
        self.last_gesture = Some((gesture.clone(), now));
        self.repeat_count
    }

    /// Whether the given key is configured to act as the Verbatim modifier.
    fn is_verbatim_modifier(&self, vk: u16, extended: bool) -> bool {
        (self.config.caps_lock && vk == VK_CAPITAL)
            || (self.config.insert && vk == VK_INSERT && extended)
            || (self.config.numpad_insert && vk == VK_INSERT && !extended)
    }

    /// Builds the normalized gesture identifier for the key going down, from
    /// the held modifiers (generalized to their generic names) plus the main
    /// key name. Returns `None` only if the identifier fails to parse, which
    /// the well-formed names here never do.
    fn build_gesture(&self, current_key: KeyCode, main_name: &str) -> Option<GestureId> {
        let mut parts: Vec<&str> = Vec::with_capacity(self.held_modifiers.len() + 1);
        for &(vk, extended) in &self.held_modifiers {
            // The key going down is the main key, not one of its own
            // modifiers, even when it is a modifier held across an auto-repeat.
            if (vk, extended) == current_key {
                continue;
            }
            if self.is_verbatim_modifier(vk, extended) {
                parts.push(keys::VERBATIM_MODIFIER_NAME);
            } else if let Some(name) = generic_modifier_name(vk) {
                parts.push(name);
            }
        }
        parts.push(main_name);
        // `GestureId::parse` lowercases and sorts the parts, so order here is
        // irrelevant and chords normalize regardless of press order.
        GestureId::parse(&format!("kb:{}", parts.join("+"))).ok()
    }
}

/// Completes `decision` with what the transition reports: nothing of
/// Verbatim's own key but a bound gesture or a lock key; for a press that
/// reaches the application, now or once deferred, the text it types and
/// whether it may have edited the text; and a lock key passed to the
/// operating system, whose new state is announced, as NVDA announces it.
/// The Verbatim modifier passed in share mode reports neither: the screen
/// reader behind Verbatim decides whether it reaches the operating system.
fn report(event: KeyEvent, source: KeySource, decision: &mut Decision) {
    if source == KeySource::Own {
        decision.speech = None;
        decision.observed = None;
    }
    let reaches = matches!(decision.verdict, Verdict::Pass | Verdict::Defer(_));
    decision.typing =
        if event.pressed && reaches && !decision.shared_modifier && source == KeySource::User {
            if decision.observed.is_none() && may_edit(event.vk) {
                PressReport::TranslatedOrPassed
            } else {
                PressReport::Translated
            }
        } else {
            PressReport::Nothing
        };
    decision.toggle =
        (event.pressed && decision.verdict == Verdict::Pass && !decision.shared_modifier)
            .then(|| ToggleKey::from_vk(event.vk))
            .flatten();
}

/// Whether `vk`, passed to the application without typing text, may edit
/// the text or move the caret, so the hook reports it and the reducer
/// watches for its evidence: any key but a modifier, which does nothing
/// alone, a lock key, whose new state is announced instead, and a key
/// Windows gives a meaning outside the text (Pause, Print Screen, Sleep,
/// and the browser, volume, media and launch keys). NVDA says nothing for
/// any of these; a watch for one would only wait out its bound.
fn may_edit(vk: u16) -> bool {
    !is_normal_modifier(vk)
        && ToggleKey::from_vk(vk).is_none()
        && !matches!(vk, 0x13 | 0x2C | 0x5F | 0xA6..=0xB7)
}

/// Maps a left/right modifier variant to its generic name, or `None` if the
/// key is not a normal modifier. Windows keys generalize to `leftwindows`,
/// matching the key-name table.
fn generic_modifier_name(vk: u16) -> Option<&'static str> {
    match vk {
        0x11 | 0xA2 | 0xA3 => Some("control"),
        0x10 | 0xA0 | 0xA1 => Some("shift"),
        0x12 | 0xA4 | 0xA5 => Some("alt"),
        0x5B | 0x5C => Some("leftwindows"),
        _ => None,
    }
}

/// How long before `now` the time `then` was, both in microseconds on the
/// run's shared clock: none when `then` is not earlier.
fn since(then: u64, now: u64) -> Duration {
    Duration::from_micros(now.saturating_sub(then))
}

/// Whether the virtual key is one of the numpad's operator keys: multiply,
/// plus, minus, or divide.
fn is_numpad_operator(vk: u16) -> bool {
    matches!(vk, 0x6A | 0x6B | 0x6D | 0x6F)
}

/// Whether the virtual key is one of the normal (non-Verbatim) modifiers.
fn is_normal_modifier(vk: u16) -> bool {
    generic_modifier_name(vk).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a map of bound gestures from raw identifiers.
    fn map_of(ids: &[&str]) -> GestureMap {
        GestureMap::new(ids.iter().map(|id| GestureId::parse(id).expect("valid id")))
    }

    /// A time on the run's shared clock, in microseconds, for a test to
    /// start from.
    const START: u64 = 1_000_000_000;

    /// `n` milliseconds, in microseconds.
    const fn ms(n: u64) -> u64 {
        n * 1_000
    }

    fn machine(config: DecisionConfig, ids: &[&str]) -> DecisionMachine {
        DecisionMachine::new(config, map_of(ids))
    }

    fn down(vk: u16, extended: bool) -> KeyEvent {
        KeyEvent {
            vk,
            scan_code: 0,
            extended,
            injected: false,
            pressed: true,
        }
    }

    fn up(vk: u16, extended: bool) -> KeyEvent {
        KeyEvent {
            pressed: false,
            ..down(vk, extended)
        }
    }

    /// The user's `event` at `now`, with Num Lock off, where Core's view
    /// lets no key be held.
    fn key(event: KeyEvent, now: u64) -> KeyInput {
        KeyInput {
            event,
            now,
            num_lock: false,
            source: KeySource::User,
            permit: HoldPermit::None,
        }
    }

    const CAPS: u16 = 0x14;
    const INSERT: u16 = 0x2D;
    const V: u16 = 0x56;
    const Z: u16 = 0x5A;
    const A: u16 = 0x41;
    const CONTROL: u16 = 0x11;

    #[test]
    fn every_key_press_cancels_speech_bound_or_not() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = START;
        // A typed letter, a lone Control, the Verbatim modifier, and a bound
        // gesture all cancel; releases do nothing.
        for (vk, extended) in [(A, false), (CONTROL, false), (CAPS, false), (V, false)] {
            assert_eq!(
                m.on_key(key(down(vk, extended), t)).speech,
                Some(KeySpeechEffect::Cancel),
                "key {vk:#x}"
            );
        }
        assert_eq!(m.on_key(key(up(V, false), t)).speech, None);
        // The volume keys and a key Windows does not know leave speech alone.
        assert_eq!(m.on_key(key(down(0xAE, true), t)).speech, None);
        assert_eq!(m.on_key(key(down(0xFF, false), t)).speech, None);
    }

    #[test]
    fn with_the_typed_character_interrupt_off_typing_and_shift_leave_speech_alone() {
        let config = DecisionConfig {
            interrupt_for_characters: false,
            ..DecisionConfig::default()
        };
        let mut m = machine(config, &[]);
        let t = START;
        assert_eq!(m.on_key(key(down(A, false), t)).speech, None);
        m.on_key(key(up(A, false), t));
        assert_eq!(m.on_key(key(down(0x20, false), t)).speech, None);
        m.on_key(key(up(0x20, false), t));
        // Shift no longer pauses, and Shift with a letter is still typing.
        assert_eq!(m.on_key(key(down(0xA0, false), t)).speech, None);
        assert_eq!(m.on_key(key(down(A, false), t)).speech, None);
        m.on_key(key(up(A, false), t));
        m.on_key(key(up(0xA0, false), t));
        // A modified letter and a command key still cancel.
        m.on_key(key(down(CONTROL, false), t));
        assert_eq!(
            m.on_key(key(down(A, false), t)).speech,
            Some(KeySpeechEffect::Cancel)
        );
        m.on_key(key(up(A, false), t));
        m.on_key(key(up(CONTROL, false), t));
        assert_eq!(
            m.on_key(key(down(0x0D, false), t)).speech,
            Some(KeySpeechEffect::Cancel)
        );
    }

    #[test]
    fn with_the_enter_interrupt_off_enter_leaves_speech_alone() {
        let config = DecisionConfig {
            interrupt_for_enter: false,
            ..DecisionConfig::default()
        };
        let mut m = machine(config, &[]);
        let t = START;
        assert_eq!(m.on_key(key(down(0x0D, false), t)).speech, None);
        m.on_key(key(up(0x0D, false), t));
        assert_eq!(
            m.on_key(key(down(A, false), t)).speech,
            Some(KeySpeechEffect::Cancel)
        );
    }

    #[test]
    fn with_num_lock_on_numpad_plus_types_its_character() {
        let mut m = machine(DecisionConfig::default(), &["kb:numpadplus"]);
        let t = START;
        let plus = 0x6B;
        assert_eq!(
            m.on_key(key(down(plus, false), t)).verdict,
            Verdict::Swallow
        );
        m.on_key(key(up(plus, false), t));
        let decision = m.on_key(KeyInput {
            num_lock: true,
            ..key(down(plus, false), t)
        });
        assert_eq!(decision.verdict, Verdict::Pass);
        assert!(decision.emitted.is_none());
        assert_eq!(m.on_key(key(up(plus, false), t)).verdict, Verdict::Pass);
    }

    #[test]
    fn an_observed_gesture_passes_and_is_reported() {
        let map = GestureMap::new([GestureId::parse("kb:v+verbatim").expect("valid id")])
            .with_observed([GestureId::parse("kb:leftarrow").expect("valid id")]);
        let mut m = DecisionMachine::new(DecisionConfig::default(), map);
        let t = START;
        let left = 0x25;
        let decision = m.on_key(key(down(left, true), t));
        assert_eq!(decision.verdict, Verdict::Pass);
        assert!(decision.emitted.is_none());
        let observed = decision.observed.expect("observed");
        assert_eq!(observed.gesture.as_str(), "kb:leftarrow");
        assert_eq!(observed.repeat, 0);
        assert_eq!(m.on_key(key(up(left, true), t)).verdict, Verdict::Pass);
        // The numpad twin is a different key and is not observed.
        assert!(m.on_key(key(down(left, false), t)).observed.is_none());
    }

    #[test]
    fn shift_alone_toggles_pause_once_per_press() {
        let mut m = machine(DecisionConfig::default(), &[]);
        let t = START;
        let left_shift = 0xA0;
        assert_eq!(
            m.on_key(key(down(left_shift, false), t)).speech,
            Some(KeySpeechEffect::TogglePause)
        );
        // Held: Windows repeats the press, which must not toggle again.
        assert_eq!(m.on_key(key(down(left_shift, false), t)).speech, None);
        m.on_key(key(up(left_shift, false), t));
        assert_eq!(
            m.on_key(key(down(left_shift, false), t)).speech,
            Some(KeySpeechEffect::TogglePause)
        );
    }

    #[test]
    fn caps_then_v_emits_and_swallows_all_four_transitions() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = START;

        let caps_down = m.on_key(key(down(CAPS, false), t));
        assert_eq!(caps_down.verdict, Verdict::Swallow);
        assert!(caps_down.emitted.is_none());

        let v_down = m.on_key(key(down(V, false), t));
        assert_eq!(v_down.verdict, Verdict::Swallow);
        let emitted = v_down.emitted.expect("gesture emitted");
        assert_eq!(emitted.gesture.as_str(), "kb:v+verbatim");

        assert_eq!(m.on_key(key(up(V, false), t)).verdict, Verdict::Swallow);
        assert_eq!(m.on_key(key(up(CAPS, false), t)).verdict, Verdict::Swallow);
    }

    #[test]
    fn double_tap_within_timeout_toggles_caps_lock_through() {
        let mut m = machine(DecisionConfig::default(), &[]);
        let t = START;

        // First tap acts as the modifier and is swallowed.
        assert_eq!(
            m.on_key(key(down(CAPS, false), t)).verdict,
            Verdict::Swallow
        );
        assert_eq!(m.on_key(key(up(CAPS, false), t)).verdict, Verdict::Swallow);

        // Second tap, comfortably inside the window, passes through both
        // transitions so the OS toggles caps lock.
        let t2 = t + ms(100);
        let second = m.on_key(key(down(CAPS, false), t2));
        assert_eq!(second.verdict, Verdict::Pass);
        assert!(!second.shared_modifier, "Verbatim alone decides its fate");
        assert_eq!(m.on_key(key(up(CAPS, false), t2)).verdict, Verdict::Pass);

        // In share mode the double tap passes for the screen reader behind
        // Verbatim, which decides whether it reaches the operating system.
        let mut m = machine(
            DecisionConfig {
                share_modifier: true,
                ..DecisionConfig::default()
            },
            &[],
        );
        m.on_key(key(down(CAPS, false), t));
        m.on_key(key(up(CAPS, false), t));
        let second = m.on_key(key(down(CAPS, false), t2));
        assert_eq!(second.verdict, Verdict::Pass);
        assert!(second.shared_modifier);
    }

    #[test]
    fn tap_then_wait_past_timeout_acts_as_modifier_again() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = START;

        assert_eq!(
            m.on_key(key(down(CAPS, false), t)).verdict,
            Verdict::Swallow
        );
        assert_eq!(m.on_key(key(up(CAPS, false), t)).verdict, Verdict::Swallow);

        // Beyond the timeout, caps acts as the modifier again.
        let t2 = t + ms(600);
        assert_eq!(
            m.on_key(key(down(CAPS, false), t2)).verdict,
            Verdict::Swallow
        );
        let v = m.on_key(key(down(V, false), t2));
        assert_eq!(v.verdict, Verdict::Swallow);
        assert_eq!(
            v.emitted.expect("emitted").gesture.as_str(),
            "kb:v+verbatim"
        );
    }

    #[test]
    fn triple_tap_second_passes_and_third_is_modifier() {
        let mut m = machine(DecisionConfig::default(), &[]);
        let t = START;

        assert_eq!(
            m.on_key(key(down(CAPS, false), t)).verdict,
            Verdict::Swallow
        );
        assert_eq!(m.on_key(key(up(CAPS, false), t)).verdict, Verdict::Swallow);

        let t2 = t + ms(100);
        assert_eq!(m.on_key(key(down(CAPS, false), t2)).verdict, Verdict::Pass);
        assert_eq!(m.on_key(key(up(CAPS, false), t2)).verdict, Verdict::Pass);

        // The passed-through tap does not arm another passthrough; the next
        // press is the modifier again.
        let t3 = t2 + ms(100);
        assert_eq!(
            m.on_key(key(down(CAPS, false), t3)).verdict,
            Verdict::Swallow
        );
    }

    #[test]
    fn extended_insert_acts_as_modifier() {
        // The modifier always names itself `verbatim` in the gesture, whether
        // caps lock, insert, or numpad insert is the physical key.
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = START;
        assert_eq!(
            m.on_key(key(down(INSERT, true), t)).verdict,
            Verdict::Swallow
        );
        let v = m.on_key(key(down(V, false), t));
        assert_eq!(v.verdict, Verdict::Swallow);
        assert_eq!(
            v.emitted.expect("emitted").gesture.as_str(),
            "kb:v+verbatim"
        );
    }

    #[test]
    fn numpad_insert_acts_as_modifier_and_is_distinct() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = START;
        // Non-extended insert is the numpad insert.
        assert_eq!(
            m.on_key(key(down(INSERT, false), t)).verdict,
            Verdict::Swallow
        );
        let v = m.on_key(key(down(V, false), t));
        assert_eq!(v.verdict, Verdict::Swallow);
        let emitted = v.emitted.expect("emitted");
        assert_eq!(emitted.gesture.as_str(), "kb:v+verbatim");
        assert_eq!(emitted.repeat, 0);

        // It is told apart from the extended Insert: with only that one a
        // modifier, the numpad Insert passes and V is an ordinary key.
        let config = DecisionConfig {
            numpad_insert: false,
            ..DecisionConfig::default()
        };
        let mut m = machine(config, &["kb:v+verbatim"]);
        assert_eq!(m.on_key(key(down(INSERT, false), t)).verdict, Verdict::Pass);
        let v = m.on_key(key(down(V, false), t));
        assert_eq!(v.verdict, Verdict::Pass);
        assert!(v.emitted.is_none());
    }

    #[test]
    fn insert_disabled_when_only_numpad_configured() {
        let config = DecisionConfig {
            caps_lock: false,
            insert: false,
            numpad_insert: true,
            ..DecisionConfig::default()
        };
        let mut m = machine(config, &["kb:v+verbatim"]);
        let t = START;
        // Extended insert is not a modifier here, so it passes through and V
        // is an ordinary keystroke.
        assert_eq!(m.on_key(key(down(INSERT, true), t)).verdict, Verdict::Pass);
        let v = m.on_key(key(down(V, false), t));
        assert_eq!(v.verdict, Verdict::Pass);
        assert!(v.emitted.is_none());
    }

    #[test]
    fn verbatim_plus_unbound_key_passes_through_bare() {
        // NVDA behavior: the modifier stays swallowed, but an unbound
        // companion key reaches the application as a bare keypress, so an
        // unrecognized chord is not eaten.
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = START;
        assert_eq!(
            m.on_key(key(down(CAPS, false), t)).verdict,
            Verdict::Swallow
        );
        let z = m.on_key(key(down(Z, false), t));
        assert_eq!(z.verdict, Verdict::Pass);
        assert!(z.emitted.is_none());
        assert_eq!(m.on_key(key(up(Z, false), t)).verdict, Verdict::Pass);
        // The bound chord still works after the passed-through key, and the
        // modifier's own transitions remain swallowed throughout.
        let v = m.on_key(key(down(V, false), t));
        assert_eq!(v.verdict, Verdict::Swallow);
        assert_eq!(
            v.emitted.expect("emitted").gesture.as_str(),
            "kb:v+verbatim"
        );
        assert_eq!(m.on_key(key(up(V, false), t)).verdict, Verdict::Swallow);
        assert_eq!(m.on_key(key(up(CAPS, false), t)).verdict, Verdict::Swallow);
    }

    #[test]
    fn control_verbatim_key_builds_normalized_chord() {
        let mut m = machine(DecisionConfig::default(), &["kb:control+v+verbatim"]);
        let t = START;
        // Control alone passes through and is tracked as a held modifier.
        assert_eq!(
            m.on_key(key(down(CONTROL, false), t)).verdict,
            Verdict::Pass
        );
        assert_eq!(
            m.on_key(key(down(CAPS, false), t)).verdict,
            Verdict::Swallow
        );
        let v = m.on_key(key(down(V, false), t));
        assert_eq!(v.verdict, Verdict::Swallow);
        assert_eq!(
            v.emitted.expect("emitted").gesture.as_str(),
            "kb:control+v+verbatim"
        );
    }

    #[test]
    fn generalized_modifier_uses_generic_name() {
        // A right-control variant must generalize to `control`.
        let mut m = machine(DecisionConfig::default(), &["kb:control+v+verbatim"]);
        let t = START;
        assert_eq!(m.on_key(key(down(0xA3, false), t)).verdict, Verdict::Pass); // right control
        assert_eq!(
            m.on_key(key(down(CAPS, false), t)).verdict,
            Verdict::Swallow
        );
        let v = m.on_key(key(down(V, false), t));
        assert_eq!(
            v.emitted.expect("emitted").gesture.as_str(),
            "kb:control+v+verbatim"
        );
    }

    #[test]
    fn injected_events_are_treated_like_physical() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = START;
        let caps = KeyEvent {
            injected: true,
            ..down(CAPS, false)
        };
        let v = KeyEvent {
            injected: true,
            ..down(V, false)
        };
        assert_eq!(m.on_key(key(caps, t)).verdict, Verdict::Swallow);
        let decision = m.on_key(key(v, t));
        assert_eq!(decision.verdict, Verdict::Swallow);
        let emitted = decision.emitted.expect("emitted");
        assert_eq!(emitted.gesture.as_str(), "kb:v+verbatim");
        assert_eq!(emitted.repeat, 0);
    }

    #[test]
    fn trapped_key_up_swallowed_while_unrelated_key_up_passes() {
        let mut m = machine(DecisionConfig::default(), &[]);
        let t = START;

        // An ordinary keystroke with no modifier passes on the way down and up.
        assert_eq!(m.on_key(key(down(A, false), t)).verdict, Verdict::Pass);
        assert_eq!(m.on_key(key(up(A, false), t)).verdict, Verdict::Pass);

        // The modifier is trapped on the way down and again on the way up.
        assert_eq!(
            m.on_key(key(down(CAPS, false), t)).verdict,
            Verdict::Swallow
        );
        assert_eq!(m.on_key(key(up(CAPS, false), t)).verdict, Verdict::Swallow);
    }

    #[test]
    fn unknown_vk_passes_through_even_with_modifier_held() {
        let mut m = machine(DecisionConfig::default(), &[]);
        let t = START;
        assert_eq!(
            m.on_key(key(down(CAPS, false), t)).verdict,
            Verdict::Swallow
        );
        // 0x07 is undefined and unnamed; it passes through.
        assert_eq!(m.on_key(key(down(0x07, false), t)).verdict, Verdict::Pass);
    }

    #[test]
    fn solo_bound_gesture_fires_without_modifier() {
        // No Verbatim modifier held, but F6 is bound as a solo gesture.
        let mut m = machine(DecisionConfig::default(), &["kb:f6"]);
        let t = START;
        let f6 = m.on_key(key(down(0x75, false), t)); // VK_F6
        assert_eq!(f6.verdict, Verdict::Swallow);
        assert_eq!(f6.emitted.expect("emitted").gesture.as_str(), "kb:f6");
        assert_eq!(m.on_key(key(up(0x75, false), t)).verdict, Verdict::Swallow);
    }

    #[test]
    fn modifier_only_gesture_can_be_bound() {
        // Binding the modifier alone still emits while swallowing.
        let mut m = machine(DecisionConfig::default(), &["kb:verbatim"]);
        let t = START;
        let caps = m.on_key(key(down(CAPS, false), t));
        assert_eq!(caps.verdict, Verdict::Swallow);
        assert_eq!(
            caps.emitted.expect("emitted").gesture.as_str(),
            "kb:verbatim"
        );
    }

    #[test]
    fn share_mode_passes_modifier_transitions_but_chords_still_fire() {
        // With share_modifier on, the modifier's own down and up pass down
        // the hook chain (for a screen reader hooked behind Verbatim), an
        // unbound companion key passes so that reader can form the chord,
        // and bound Verbatim chords still fire and are swallowed.
        let config = DecisionConfig {
            share_modifier: true,
            ..DecisionConfig::default()
        };
        let mut m = machine(config, &["kb:v+verbatim"]);
        let t = START;

        let caps = m.on_key(key(down(CAPS, false), t));
        assert_eq!(caps.verdict, Verdict::Pass);
        assert!(caps.shared_modifier, "passed as the shared modifier");
        let z = m.on_key(key(down(Z, false), t));
        assert!(!z.shared_modifier);
        assert_eq!(z.verdict, Verdict::Pass);
        assert!(z.emitted.is_none());
        assert_eq!(m.on_key(key(up(Z, false), t)).verdict, Verdict::Pass);

        let v = m.on_key(key(down(V, false), t));
        assert_eq!(v.verdict, Verdict::Swallow);
        assert_eq!(
            v.emitted.expect("emitted").gesture.as_str(),
            "kb:v+verbatim"
        );
        assert_eq!(m.on_key(key(up(V, false), t)).verdict, Verdict::Swallow);

        assert_eq!(m.on_key(key(up(CAPS, false), t)).verdict, Verdict::Pass);
    }

    #[test]
    fn auto_repeat_of_modifier_does_not_break_chords() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = START;
        // Caps auto-repeats (repeated down with no up).
        assert_eq!(
            m.on_key(key(down(CAPS, false), t)).verdict,
            Verdict::Swallow
        );
        assert_eq!(
            m.on_key(key(down(CAPS, false), t)).verdict,
            Verdict::Swallow
        );
        let v = m.on_key(key(down(V, false), t));
        assert_eq!(v.verdict, Verdict::Swallow);
        assert_eq!(
            v.emitted.expect("emitted").gesture.as_str(),
            "kb:v+verbatim"
        );
    }

    /// Presses and releases `kb:v+verbatim` once as a genuine (non-repeated)
    /// press at time `t`, returning the repeat count it carried.
    fn press_v_verbatim(m: &mut DecisionMachine, t: u64) -> u8 {
        m.on_key(key(down(CAPS, false), t));
        let v = m.on_key(key(down(V, false), t));
        let repeat = v.emitted.expect("emitted").repeat;
        m.on_key(key(up(V, false), t));
        m.on_key(key(up(CAPS, false), t));
        repeat
    }

    #[test]
    fn first_press_carries_repeat_zero() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = START;
        assert_eq!(press_v_verbatim(&mut m, t), 0);
    }

    #[test]
    fn second_press_within_window_increments_repeat() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = START;
        assert_eq!(press_v_verbatim(&mut m, t), 0);
        let t2 = t + ms(100);
        assert_eq!(press_v_verbatim(&mut m, t2), 1);
    }

    #[test]
    fn third_press_within_window_increments_again() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = START;
        assert_eq!(press_v_verbatim(&mut m, t), 0);
        let t2 = t + ms(100);
        assert_eq!(press_v_verbatim(&mut m, t2), 1);
        let t3 = t2 + ms(100);
        assert_eq!(press_v_verbatim(&mut m, t3), 2);
    }

    #[test]
    fn press_after_window_elapses_resets_to_zero() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = START;
        assert_eq!(press_v_verbatim(&mut m, t), 0);
        let t2 = t + ms(100);
        assert_eq!(press_v_verbatim(&mut m, t2), 1);
        // Well past the 500 ms multi-press timeout since the second press.
        let t3 = t2 + ms(600);
        assert_eq!(press_v_verbatim(&mut m, t3), 0);
    }

    #[test]
    fn a_different_gesture_resets_the_streak() {
        let mut m = machine(
            DecisionConfig::default(),
            &["kb:v+verbatim", "kb:a+verbatim"],
        );
        let t = START;
        assert_eq!(press_v_verbatim(&mut m, t), 0);

        // Press a different bound gesture within the window.
        let t2 = t + ms(100);
        m.on_key(key(down(CAPS, false), t2));
        let a = m.on_key(key(down(A, false), t2));
        assert_eq!(a.emitted.expect("emitted").repeat, 0);
        m.on_key(key(up(A, false), t2));
        m.on_key(key(up(CAPS, false), t2));

        // verbatim+v again, still within the original window: the streak
        // was broken by the different gesture, so this is a fresh press.
        let t3 = t2 + ms(100);
        assert_eq!(press_v_verbatim(&mut m, t3), 0);
    }

    #[test]
    fn an_unbound_key_between_presses_resets_the_streak_but_a_modifier_does_not() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = START;
        assert_eq!(press_v_verbatim(&mut m, t), 0);
        let t2 = t + ms(50);
        assert_eq!(
            m.on_key(key(down(CONTROL, false), t2)).verdict,
            Verdict::Pass
        );
        m.on_key(key(up(CONTROL, false), t2));
        assert_eq!(
            press_v_verbatim(&mut m, t2 + ms(50)),
            1,
            "a lone modifier does not end the streak"
        );
        let t3 = t2 + ms(100);
        assert_eq!(m.on_key(key(down(A, false), t3)).verdict, Verdict::Pass);
        m.on_key(key(up(A, false), t3));
        assert_eq!(
            press_v_verbatim(&mut m, t3 + ms(50)),
            0,
            "an unbound key ends it, as in NVDA"
        );
    }

    #[test]
    fn auto_repeat_does_not_advance_the_repeat_count() {
        // Holding the gesture's own key down auto-repeats its key-down with
        // no intervening key-up (unlike press_v_verbatim's up/down pairs,
        // which are genuine separate presses). NVDA does not treat
        // auto-repeat as a multi-press (deliberately unlike NVDA), so every
        // auto-repeated emission must carry the same count as the press
        // that started the hold.
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = START;
        m.on_key(key(down(CAPS, false), t));

        let first = m.on_key(key(down(V, false), t));
        assert_eq!(first.emitted.expect("emitted").repeat, 0);

        // Auto-repeat: V goes down again with no key-up in between.
        let repeat1 = m.on_key(key(down(V, false), t));
        assert_eq!(repeat1.emitted.expect("emitted").repeat, 0);
        let repeat2 = m.on_key(key(down(V, false), t));
        assert_eq!(repeat2.emitted.expect("emitted").repeat, 0);

        m.on_key(key(up(V, false), t));
        m.on_key(key(up(CAPS, false), t));

        // A genuine second press afterward, within the window, still counts
        // as the second press (1), not a fourth (3): the auto-repeats above
        // never advanced the count.
        let t2 = t + ms(50);
        assert_eq!(press_v_verbatim(&mut m, t2), 1);
    }

    #[test]
    fn repeat_count_saturates_instead_of_overflowing() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let mut t = START;
        let mut last = 0;
        for _ in 0..=300 {
            last = press_v_verbatim(&mut m, t);
            t += ms(10);
        }
        assert_eq!(last, u8::MAX);
    }

    const RIGHT: u16 = 0x27;
    const X: u16 = 0x58;
    const ESCAPE: u16 = 0x1B;

    /// A machine binding Verbatim+Right Arrow, holding Right Arrow and
    /// observing Escape.
    fn holding_machine() -> DecisionMachine {
        let id = |raw| GestureId::parse(raw).expect("valid id");
        DecisionMachine::new(
            DecisionConfig::default(),
            GestureMap::new([id("kb:rightarrow+verbatim")])
                .with_observed([id("kb:escape")])
                .with_held([id("kb:rightarrow")]),
        )
    }

    /// The user's `event` at `now`, with `permit`.
    fn with(permit: HoldPermit, event: KeyEvent, now: u64) -> KeyInput {
        KeyInput {
            permit,
            ..key(event, now)
        }
    }

    /// The verdict on the user's `event` at `now` with `permit`.
    fn verdict(m: &mut DecisionMachine, permit: HoldPermit, event: KeyEvent, now: u64) -> Verdict {
        m.on_key(with(permit, event, now)).verdict
    }

    const HOLD: HoldPermit = HoldPermit::HoldAndDefer;

    /// While a held key has not reached the application, every later key
    /// that would pass, a typed letter and its release included, is
    /// deferred behind it in the order pressed, and the letter still
    /// reports what it types; a later caret key is held in its own turn.
    /// Once the keys passed on come back, up to the last one held back,
    /// keys pass again; one passed on with a later place clears those
    /// before it, which Windows may have dropped.
    #[test]
    fn keys_behind_a_held_key_are_deferred_in_order_until_it_is_passed_on() {
        let mut m = holding_machine();
        let t = START;
        let held = m.on_key(with(HOLD, down(RIGHT, true), t));
        assert_eq!(held.verdict, Verdict::Hold(1));
        assert_eq!(held.typing, PressReport::Nothing);
        assert_eq!(verdict(&mut m, HOLD, up(RIGHT, true), t), Verdict::Swallow);
        let x = m.on_key(with(HOLD, down(X, false), t));
        assert_eq!(x.verdict, Verdict::Defer(2));
        assert_eq!(x.typing, PressReport::TranslatedOrPassed);
        assert_eq!(
            verdict(&mut m, HOLD, down(RIGHT, true), t),
            Verdict::Hold(3)
        );
        assert_eq!(verdict(&mut m, HOLD, up(X, false), t), Verdict::Defer(4));
        // The first Right Arrow and the letter come back passed on: the
        // keys behind them still wait.
        m.passed_on(1);
        m.passed_on(2);
        assert_eq!(verdict(&mut m, HOLD, down(X, false), t), Verdict::Defer(5));
        // The last key held back comes back, clearing the second Right
        // Arrow and the letter's release though they never came: a key
        // pressed now passes.
        m.passed_on(5);
        assert!(!m.has_outstanding());
        assert_eq!(verdict(&mut m, HOLD, down(X, false), t), Verdict::Pass);
        assert_eq!(verdict(&mut m, HOLD, up(X, false), t), Verdict::Pass);
        // The modifiers and releases sent around a held key carry no
        // place, and clear nothing.
        assert_eq!(
            verdict(&mut m, HOLD, down(RIGHT, true), t),
            Verdict::Hold(6)
        );
        m.passed_on(0);
        assert_eq!(verdict(&mut m, HOLD, down(X, false), t), Verdict::Defer(7));
    }

    /// A lock key pressed while a held key has not reached the application
    /// passes at once, press and release, and is not deferred: its state is
    /// announced from Windows' own, and Windows translates the numpad's
    /// keys by Num Lock's state before the hook sees them. Caps Lock, the
    /// Verbatim key by default, reaches the application on a double press,
    /// whose first press is the Verbatim key's and swallowed. A letter after
    /// them is still deferred.
    #[test]
    fn a_lock_key_passes_at_once_behind_a_held_key() {
        let mut m = holding_machine();
        let t = START;
        assert_eq!(
            verdict(&mut m, HOLD, down(RIGHT, true), t),
            Verdict::Hold(1)
        );
        let mut decided = Vec::new();
        for (vk, extended, presses) in [(CAPS, false, 2), (0x90, true, 1), (0x91, false, 1)] {
            for _ in 0..presses {
                for event in [down(vk, extended), up(vk, extended)] {
                    let decision = m.on_key(with(HOLD, event, t + ms(10)));
                    decided.push((vk, event.pressed, decision.verdict, decision.toggle));
                }
            }
        }
        let (swallow, pass) = (Verdict::Swallow, Verdict::Pass);
        assert_eq!(
            decided,
            [
                (CAPS, true, swallow, None),
                (CAPS, false, swallow, None),
                (CAPS, true, pass, Some(ToggleKey::CapsLock)),
                (CAPS, false, pass, None),
                (0x90, true, pass, Some(ToggleKey::NumLock)),
                (0x90, false, pass, None),
                (0x91, true, pass, Some(ToggleKey::ScrollLock)),
                (0x91, false, pass, None),
            ]
        );
        assert_eq!(verdict(&mut m, HOLD, down(X, false), t), Verdict::Defer(2));
    }

    /// While Core's view lets keys be held and the focus has text, a caret
    /// key in the held set is held back, press and release, and reported
    /// observed; a key observed but not held (Escape) still passes; with
    /// the focus without text the caret key passes and is reported
    /// observed, as before.
    #[test]
    fn a_caret_key_is_held_while_holding_is_on() {
        let mut m = holding_machine();
        let t = START;
        let held = m.on_key(with(HOLD, down(RIGHT, true), t));
        assert_eq!(held.verdict, Verdict::Hold(1));
        assert_eq!(
            held.observed.map(|observed| observed.gesture),
            Some(GestureId::parse("kb:rightarrow").expect("valid id"))
        );
        assert_eq!(verdict(&mut m, HOLD, up(RIGHT, true), t), Verdict::Swallow);
        m.passed_on(1);
        let escape = m.on_key(with(HOLD, down(ESCAPE, false), t));
        assert_eq!(escape.verdict, Verdict::Pass);
        assert!(escape.observed.is_some());
        assert_eq!(verdict(&mut m, HOLD, up(ESCAPE, false), t), Verdict::Pass);
        let defer = HoldPermit::Defer;
        let passed = m.on_key(with(defer, down(RIGHT, true), t));
        assert_eq!(passed.verdict, Verdict::Pass);
        assert!(passed.observed.is_some());
        assert_eq!(verdict(&mut m, defer, up(RIGHT, true), t), Verdict::Pass);
    }

    #[test]
    fn a_key_with_a_meaning_outside_the_text_is_not_watched() {
        // Pause, Print Screen, Sleep, Volume Up, Play/Pause, Launch Mail.
        for vk in [0x13, 0x2C, 0x5F, 0xAF, 0xB3, 0xB4] {
            assert!(!may_edit(vk), "{vk:#04x}");
        }
        // Left Control and Caps Lock, as before.
        for vk in [0xA2, 0x14] {
            assert!(!may_edit(vk), "{vk:#04x}");
        }
        // Keys Verbatim does not know that may move the caret: bash's
        // Control+A and Alt+F, and F3, Find Next.
        for vk in [0x41, 0x46, 0x72] {
            assert!(may_edit(vk), "{vk:#04x}");
        }
    }

    #[test]
    fn a_held_keys_release_is_swallowed_after_its_pass_on() {
        let mut m = holding_machine();
        assert_eq!(
            verdict(&mut m, HOLD, down(RIGHT, true), START),
            Verdict::Hold(1)
        );
        m.passed_on(1);
        assert_eq!(
            verdict(&mut m, HOLD, up(RIGHT, true), START),
            Verdict::Swallow
        );
    }

    /// Verbatim+Right Arrow pressed twice, its count reaching 1; then Right
    /// Arrow held in text, which ends the streak, the Verbatim modifier
    /// pressed, and Right's auto-repeat completing the gesture: that
    /// auto-repeat is a fresh press, counted 0 rather than carrying the
    /// streak's 1, and the auto-repeats after it carry its 0.
    #[test]
    fn a_held_keys_auto_repeat_completing_a_gesture_counts_as_a_fresh_press() {
        let mut m = holding_machine();
        let repeat = |m: &mut DecisionMachine, now| {
            m.on_key(with(HOLD, down(RIGHT, true), now))
                .emitted
                .expect("emitted")
                .repeat
        };
        let mut t = START;
        for expected in [0, 1] {
            m.on_key(with(HOLD, down(INSERT, true), t));
            assert_eq!(repeat(&mut m, t), expected);
            m.on_key(with(HOLD, up(RIGHT, true), t));
            m.on_key(with(HOLD, up(INSERT, true), t));
            t += ms(33);
        }
        assert_eq!(
            verdict(&mut m, HOLD, down(RIGHT, true), t),
            Verdict::Hold(1)
        );
        m.on_key(with(HOLD, down(INSERT, true), t + ms(33)));
        assert_eq!(repeat(&mut m, t + ms(66)), 0);
        assert_eq!(repeat(&mut m, t + ms(99)), 0);
    }

    /// Right Arrow held in a search box whose results list takes the focus:
    /// its auto-repeat after holding switches off passes, and so does its
    /// release, so the list and Windows see the key come up.
    #[test]
    fn a_held_keys_auto_repeat_passed_after_holding_switches_off_has_its_release_passed() {
        let mut m = holding_machine();
        assert_eq!(
            verdict(&mut m, HOLD, down(RIGHT, true), START),
            Verdict::Hold(1)
        );
        m.passed_on(1);
        let none = HoldPermit::None;
        assert_eq!(
            verdict(&mut m, none, down(RIGHT, true), START),
            Verdict::Pass
        );
        assert_eq!(verdict(&mut m, none, up(RIGHT, true), START), Verdict::Pass);
    }

    /// Verbatim+Right Arrow, the modifier let go before Right: Right's
    /// auto-repeat passes, and so does its release.
    #[test]
    fn a_gestures_key_whose_auto_repeat_passes_after_the_modifier_is_let_go_has_its_release_passed()
    {
        let mut m = holding_machine();
        let none = HoldPermit::None;
        assert_eq!(
            verdict(&mut m, none, down(INSERT, true), START),
            Verdict::Swallow
        );
        assert_eq!(
            verdict(&mut m, none, down(RIGHT, true), START),
            Verdict::Swallow
        );
        assert_eq!(
            verdict(&mut m, none, up(INSERT, true), START),
            Verdict::Swallow
        );
        assert_eq!(
            verdict(&mut m, none, down(RIGHT, true), START),
            Verdict::Pass
        );
        assert_eq!(verdict(&mut m, none, up(RIGHT, true), START), Verdict::Pass);
    }

    /// Right Arrow passed, then held once the focus has text: the held
    /// press is sent again with its own release, so the user's release is
    /// swallowed rather than reach the application a second time.
    #[test]
    fn a_held_press_after_one_that_passed_has_its_release_swallowed() {
        let mut m = holding_machine();
        let none = HoldPermit::None;
        assert_eq!(
            verdict(&mut m, none, down(RIGHT, true), START),
            Verdict::Pass
        );
        assert_eq!(
            verdict(&mut m, HOLD, down(RIGHT, true), START),
            Verdict::Hold(1)
        );
        assert_eq!(
            verdict(&mut m, HOLD, up(RIGHT, true), START),
            Verdict::Swallow
        );
    }

    #[test]
    fn passed_on_changes_only_the_line() {
        let mut m = holding_machine();
        m.on_key(with(HOLD, down(RIGHT, true), START));
        m.on_key(with(HOLD, down(X, false), START));
        m.on_key(with(HOLD, down(RIGHT, true), START));
        let mut expected = m.clone();
        m.passed_on(9);
        assert_eq!(m, expected);
        m.passed_on(2);
        expected.line.drain(..2);
        assert_eq!(m, expected);
    }

    /// Verbatim's own keys pass, neither held nor deferred though a key
    /// waits in line and the focus has text, and report no speech effect,
    /// observed gesture or typing.
    #[test]
    fn own_input_is_neither_held_nor_deferred_and_reports_nothing_passed() {
        let mut m = holding_machine();
        assert_eq!(
            verdict(&mut m, HOLD, down(RIGHT, true), START),
            Verdict::Hold(1)
        );
        for event in [
            down(CONTROL, false),
            up(CONTROL, false),
            down(ESCAPE, false),
        ] {
            let decision = m.on_key(KeyInput {
                source: KeySource::Own,
                ..with(HOLD, event, START)
            });
            assert_eq!(
                (
                    decision.verdict,
                    decision.speech,
                    decision.observed,
                    decision.typing
                ),
                (Verdict::Pass, None, None, PressReport::Nothing)
            );
        }
    }
}
