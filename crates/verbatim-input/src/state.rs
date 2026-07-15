//! The pure decision state machine.
//!
//! This module is the heart of input handling and is deliberately free of any
//! operating-system dependency: it turns a stream of [`KeyEvent`]s into
//! swallow-or-pass [`Decision`]s and emitted gestures, driven only by its
//! configuration, the bound-gesture snapshot, and a caller-supplied clock.
//! The hook thread ([`crate::hook`]) is the thin imperative shell that feeds
//! it real events; every behavioural rule lives here so it can be exercised
//! by ordinary unit tests with scripted key streams.
//!
//! The semantics follow NVDA's `keyboardHandler` (`internal_keyDownEvent`,
//! `internal_keyUpEvent`, and `isNVDAModifierKey`) closely. The notable
//! deliberate differences from NVDA are documented on [`DecisionMachine`].

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use verbatim_model::{GestureId, TraceId};

use crate::keys;
use crate::map::GestureMap;
use crate::{KeyDecision, KeyEvent};

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
}

impl Default for DecisionConfig {
    fn default() -> Self {
        Self {
            caps_lock: true,
            insert: true,
            numpad_insert: true,
            share_modifier: false,
            multi_press_timeout: DEFAULT_MULTI_PRESS_TIMEOUT,
        }
    }
}

/// A gesture the state machine decided to raise, tagged for tracing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmittedGesture {
    /// A freshly minted trace ID correlating this keypress with everything it
    /// causes downstream (architecture section 9).
    pub trace_id: TraceId,
    /// The bound gesture that fired.
    pub gesture: GestureId,
    /// The multi-press repeat count: 0 for the first press, 1 for the
    /// second, 2 for the third, and so on, saturating rather than
    /// overflowing. Incremented when the same gesture fires again within
    /// `multi_press_timeout` of its previous genuine press; a different
    /// gesture or the window elapsing resets it to 0. NVDA semantics —
    /// consumers dispatch on it for report-current (report, spell, copy),
    /// speak time (time, then date), and show tray list (tray, then
    /// taskbar). Auto-repeat (holding the gesture's key down, which
    /// re-fires the gesture on every OS auto-repeat tick with no
    /// intervening key-up) does not advance this count — NVDA does not
    /// treat auto-repeat as a multi-press for script-repeat purposes, and
    /// every auto-repeated emission of a held gesture carries the same
    /// count as its initiating genuine press.
    pub repeat: u8,
}

/// The outcome of feeding one [`KeyEvent`] to the state machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    /// Whether Windows should swallow or pass the transition.
    pub decision: KeyDecision,
    /// The gesture this transition raised, if any. Only ever set on a
    /// [`KeyDecision::Swallow`].
    pub emitted: Option<EmittedGesture>,
}

impl Decision {
    /// Pass the transition through, raising nothing.
    fn pass() -> Self {
        Self {
            decision: KeyDecision::Pass,
            emitted: None,
        }
    }

    /// Swallow the transition, raising nothing.
    fn swallow() -> Self {
        Self {
            decision: KeyDecision::Swallow,
            emitted: None,
        }
    }

    /// Swallow the transition and raise a gesture.
    fn swallow_emit(emitted: EmittedGesture) -> Self {
        Self {
            decision: KeyDecision::Swallow,
            emitted: Some(emitted),
        }
    }
}

/// Identity of a physical key for the trapped-key bookkeeping: the
/// virtual-key code paired with the extended flag, so an extended key and its
/// numpad twin are distinct.
type KeyCode = (u16, bool);

/// The pure keyboard decision state machine.
///
/// Construct one with [`DecisionMachine::new`], then call
/// [`on_key`](DecisionMachine::on_key) for every raw key transition. The
/// machine reads the bound-gesture set from a lock-free
/// [`ArcSwap`] snapshot on each call, so rebinding gestures is a single
/// atomic store elsewhere and never blocks the hook.
///
/// # Behaviour, following NVDA
///
/// - A Verbatim-modifier key-down is always swallowed (so caps lock never
///   toggles), unless the double-tap passthrough is active.
/// - While the modifier is held, a key-down forming a bound gesture is
///   swallowed and the gesture emitted; an unbound companion key falls
///   through to the application as a bare keypress (the modifier itself
///   stays swallowed), so unrecognized chords are not eaten.
/// - Double-tapping the modifier alone within `multi_press_timeout` passes the
///   second press-and-release through to the operating system, so caps lock
///   toggles or insert inserts.
/// - Keys swallowed on their way down are recorded as trapped and swallowed
///   again on their way up.
/// - Injected keys are processed identically to physical ones, so the control
///   plane can drive gestures with synthetic input.
/// - In share mode ([`DecisionConfig::share_modifier`]) the modifier's own
///   transitions pass down the hook chain instead of being swallowed, so a
///   screen reader hooked behind Verbatim can use the same modifier for the
///   chords Verbatim leaves unbound.
/// - Multi-press counting: each [`EmittedGesture`] carries a `repeat` count.
///   Pressing the same bound gesture again within `multi_press_timeout` of
///   its previous genuine press increments the count (0, 1, 2, ...,
///   saturating); a different gesture or the window elapsing resets it to
///   0. Holding a gesture's key down auto-repeats key-downs with no
///   intervening key-up; each auto-repeated emission carries the same count
///   as the genuine press that started the hold, matching NVDA, which does
///   not treat auto-repeat as a multi-press for script-repeat purposes.
///
/// # Deliberate differences from NVDA
///
/// - Sticky Keys interaction (NVDA's `stickyNVDAModifier` latch/lock) is not
///   modelled; that is a later accessibility-integration concern.
/// - NVDA's `passNextKeyThrough` command (a user-requested one-shot
///   passthrough counter) is not modelled; it is a separate feature.
/// - Unknown virtual keys (those [`keys::name_from_vk`] cannot name) pass
///   through unswallowed, in place of NVDA's elaborate `MapVirtualKeyEx` and
///   `GetKeyNameText` fallbacks.
pub struct DecisionMachine {
    config: DecisionConfig,
    map: Arc<ArcSwap<GestureMap>>,
    /// Modifier keys currently held down (normal modifiers and the Verbatim
    /// modifier), each as its virtual-key code and extended flag.
    current_modifiers: HashSet<KeyCode>,
    /// Keys swallowed on their way down, so their key-up is swallowed too.
    trapped: HashSet<KeyCode>,
    /// The last Verbatim-modifier key pressed with no other key pressed since;
    /// cleared the moment any other key goes down.
    last_verbatim_modifier: Option<KeyCode>,
    /// When [`last_verbatim_modifier`](Self::last_verbatim_modifier) was
    /// released, used to time the double-tap passthrough window.
    last_release: Option<Instant>,
    /// Whether the modifier's special behaviour is currently bypassed (the
    /// double-tap passthrough), until the next key-up ends the repeats.
    bypass: bool,
    /// The gesture and observation time of the most recent genuine (not
    /// auto-repeated) bound-gesture press, for multi-press counting.
    last_gesture: Option<(GestureId, Instant)>,
    /// The multi-press repeat count of
    /// [`last_gesture`](Self::last_gesture); carried unchanged across
    /// auto-repeats of the same held gesture.
    repeat_count: u8,
}

impl DecisionMachine {
    /// Creates a machine with the given configuration, reading bound gestures
    /// from `map`.
    #[must_use]
    pub fn new(config: DecisionConfig, map: Arc<ArcSwap<GestureMap>>) -> Self {
        Self {
            config,
            map,
            current_modifiers: HashSet::new(),
            trapped: HashSet::new(),
            last_verbatim_modifier: None,
            last_release: None,
            bypass: false,
            last_gesture: None,
            repeat_count: 0,
        }
    }

    /// Feeds one raw key transition and returns the swallow-or-pass decision
    /// plus any gesture it raised.
    ///
    /// `now` is the observation time; the hook passes [`Instant::now`], and
    /// tests pass a scripted clock. The bound-gesture snapshot is loaded here,
    /// lock-free.
    pub fn on_key(&mut self, event: KeyEvent, now: Instant) -> Decision {
        if event.pressed {
            self.on_down(event, now)
        } else {
            self.on_up(event, now)
        }
    }

    fn on_down(&mut self, event: KeyEvent, now: Instant) -> Decision {
        let key: KeyCode = (event.vk, event.extended);
        let mut is_verbatim = self.is_verbatim_modifier(event.vk, event.extended);

        // Double-tap passthrough: if we are already bypassing, or this is a
        // fresh press of the same lone modifier within the timeout, the key
        // serves its normal OS function instead of acting as the modifier.
        // There may be auto-repeats, so keep bypassing until the next key-up.
        let within_window = self.last_verbatim_modifier == Some(key)
            && self.last_release.is_some_and(|t| {
                now.saturating_duration_since(t) < self.config.multi_press_timeout
            });
        if self.bypass || within_window {
            self.bypass = true;
            is_verbatim = false;
        }
        // A key going down always ends the pending release window.
        self.last_release = None;

        // Track the lone-modifier candidate for the next double-tap: a
        // Verbatim modifier arms it, any other key clears it.
        self.last_verbatim_modifier = if is_verbatim { Some(key) } else { None };

        // The gesture's main key name. The Verbatim modifier names itself;
        // every other key is named from the table, and unknown keys pass
        // through unswallowed.
        let main_name = if is_verbatim {
            keys::VERBATIM_MODIFIER_NAME
        } else if let Some(name) = keys::name_from_vk(event.vk, event.extended) {
            name
        } else {
            return Decision::pass();
        };

        // Record held modifiers so later keys can form chords with them.
        if is_verbatim || is_normal_modifier(event.vk) {
            self.current_modifiers.insert(key);
        }

        if let Some(gesture) = self.build_gesture(key, main_name)
            && self.map.load().contains(&gesture)
        {
            // Auto-repeat: the OS keeps sending key-down for a held key with
            // no intervening key-up, so a still-trapped key means this
            // down is a repeat of the press already in progress, not a new
            // one. NVDA does not count auto-repeat toward a script's
            // multi-press repeat count, so the count carried on
            // `last_gesture` from the initiating genuine press is reused
            // unchanged rather than recomputed here.
            let auto_repeat = self.trapped.contains(&key);
            self.trapped.insert(key);
            if !auto_repeat {
                self.repeat_count = match &self.last_gesture {
                    Some((last, at))
                        if *last == gesture
                            && now.saturating_duration_since(*at)
                                < self.config.multi_press_timeout =>
                    {
                        self.repeat_count.saturating_add(1)
                    }
                    _ => 0,
                };
                self.last_gesture = Some((gesture.clone(), now));
            }
            return Decision::swallow_emit(EmittedGesture {
                trace_id: TraceId::mint(),
                gesture,
                repeat: self.repeat_count,
            });
        }

        // Unbound. An unbound companion key falls through to the application
        // as a bare keypress — NVDA's behavior, so an unrecognized chord
        // like verbatim+pageup still pages the app instead of being eaten.
        // The modifier key itself is normally swallowed (caps lock must not
        // toggle); in share mode its real transitions pass down the hook
        // chain instead, for a screen reader hooked behind Verbatim to see —
        // that reader is then the one that swallows it before the OS.
        if is_verbatim {
            if self.config.share_modifier {
                return Decision::pass();
            }
            self.trapped.insert(key);
            return Decision::swallow();
        }
        Decision::pass()
    }

    fn on_up(&mut self, event: KeyEvent, now: Instant) -> Decision {
        let key: KeyCode = (event.vk, event.extended);

        // Releasing the lone modifier arms the double-tap window.
        if self.last_verbatim_modifier == Some(key) {
            self.last_release = Some(now);
        }
        // Any key-up ends a bypass: there will be no more auto-repeats.
        self.bypass = false;
        self.current_modifiers.remove(&key);

        if self.trapped.remove(&key) {
            Decision::swallow()
        } else {
            Decision::pass()
        }
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
        let mut parts: Vec<&str> = Vec::with_capacity(self.current_modifiers.len() + 1);
        for &(vk, extended) in &self.current_modifiers {
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

/// Whether the virtual key is one of the normal (non-Verbatim) modifiers.
fn is_normal_modifier(vk: u16) -> bool {
    generic_modifier_name(vk).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a shared, bound-gesture snapshot from raw identifiers.
    fn map_of(ids: &[&str]) -> Arc<ArcSwap<GestureMap>> {
        let gestures = ids.iter().map(|id| GestureId::parse(id).expect("valid id"));
        Arc::new(ArcSwap::from_pointee(GestureMap::new(gestures)))
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

    const CAPS: u16 = 0x14;
    const INSERT: u16 = 0x2D;
    const V: u16 = 0x56;
    const Z: u16 = 0x5A;
    const A: u16 = 0x41;
    const CONTROL: u16 = 0x11;

    #[test]
    fn caps_then_v_emits_and_swallows_all_four_transitions() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = Instant::now();

        let caps_down = m.on_key(down(CAPS, false), t);
        assert_eq!(caps_down.decision, KeyDecision::Swallow);
        assert!(caps_down.emitted.is_none());

        let v_down = m.on_key(down(V, false), t);
        assert_eq!(v_down.decision, KeyDecision::Swallow);
        let emitted = v_down.emitted.expect("gesture emitted");
        assert_eq!(emitted.gesture.as_str(), "kb:v+verbatim");

        assert_eq!(m.on_key(up(V, false), t).decision, KeyDecision::Swallow);
        assert_eq!(m.on_key(up(CAPS, false), t).decision, KeyDecision::Swallow);
    }

    #[test]
    fn double_tap_within_timeout_toggles_caps_lock_through() {
        let mut m = machine(DecisionConfig::default(), &[]);
        let t = Instant::now();

        // First tap acts as the modifier and is swallowed.
        assert_eq!(
            m.on_key(down(CAPS, false), t).decision,
            KeyDecision::Swallow
        );
        assert_eq!(m.on_key(up(CAPS, false), t).decision, KeyDecision::Swallow);

        // Second tap, comfortably inside the window, passes through both
        // transitions so the OS toggles caps lock.
        let t2 = t + Duration::from_millis(100);
        assert_eq!(m.on_key(down(CAPS, false), t2).decision, KeyDecision::Pass);
        assert_eq!(m.on_key(up(CAPS, false), t2).decision, KeyDecision::Pass);
    }

    #[test]
    fn tap_then_wait_past_timeout_acts_as_modifier_again() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = Instant::now();

        assert_eq!(
            m.on_key(down(CAPS, false), t).decision,
            KeyDecision::Swallow
        );
        assert_eq!(m.on_key(up(CAPS, false), t).decision, KeyDecision::Swallow);

        // Beyond the timeout, caps acts as the modifier again.
        let t2 = t + Duration::from_millis(600);
        assert_eq!(
            m.on_key(down(CAPS, false), t2).decision,
            KeyDecision::Swallow
        );
        let v = m.on_key(down(V, false), t2);
        assert_eq!(v.decision, KeyDecision::Swallow);
        assert_eq!(
            v.emitted.expect("emitted").gesture.as_str(),
            "kb:v+verbatim"
        );
    }

    #[test]
    fn triple_tap_second_passes_and_third_is_modifier() {
        let mut m = machine(DecisionConfig::default(), &[]);
        let t = Instant::now();

        assert_eq!(
            m.on_key(down(CAPS, false), t).decision,
            KeyDecision::Swallow
        );
        assert_eq!(m.on_key(up(CAPS, false), t).decision, KeyDecision::Swallow);

        let t2 = t + Duration::from_millis(100);
        assert_eq!(m.on_key(down(CAPS, false), t2).decision, KeyDecision::Pass);
        assert_eq!(m.on_key(up(CAPS, false), t2).decision, KeyDecision::Pass);

        // The passed-through tap does not arm another passthrough; the next
        // press is the modifier again.
        let t3 = t2 + Duration::from_millis(100);
        assert_eq!(
            m.on_key(down(CAPS, false), t3).decision,
            KeyDecision::Swallow
        );
    }

    #[test]
    fn extended_insert_acts_as_modifier() {
        // The modifier always names itself `verbatim` in the gesture, whether
        // caps lock, insert, or numpad insert is the physical key.
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = Instant::now();
        assert_eq!(
            m.on_key(down(INSERT, true), t).decision,
            KeyDecision::Swallow
        );
        let v = m.on_key(down(V, false), t);
        assert_eq!(v.decision, KeyDecision::Swallow);
        assert_eq!(
            v.emitted.expect("emitted").gesture.as_str(),
            "kb:v+verbatim"
        );
    }

    #[test]
    fn numpad_insert_acts_as_modifier_and_is_distinct() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = Instant::now();
        // Non-extended insert is the numpad insert.
        assert_eq!(
            m.on_key(down(INSERT, false), t).decision,
            KeyDecision::Swallow
        );
        let v = m.on_key(down(V, false), t);
        assert_eq!(v.decision, KeyDecision::Swallow);
        assert!(v.emitted.is_some());
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
        let t = Instant::now();
        // Extended insert is not a modifier here, so it passes through and V
        // is an ordinary keystroke.
        assert_eq!(m.on_key(down(INSERT, true), t).decision, KeyDecision::Pass);
        let v = m.on_key(down(V, false), t);
        assert_eq!(v.decision, KeyDecision::Pass);
        assert!(v.emitted.is_none());
    }

    #[test]
    fn verbatim_plus_unbound_key_passes_through_bare() {
        // NVDA behavior: the modifier stays swallowed, but an unbound
        // companion key reaches the application as a bare keypress, so an
        // unrecognized chord is not eaten.
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = Instant::now();
        assert_eq!(
            m.on_key(down(CAPS, false), t).decision,
            KeyDecision::Swallow
        );
        let z = m.on_key(down(Z, false), t);
        assert_eq!(z.decision, KeyDecision::Pass);
        assert!(z.emitted.is_none());
        assert_eq!(m.on_key(up(Z, false), t).decision, KeyDecision::Pass);
        // The bound chord still works after the passed-through key, and the
        // modifier's own transitions remain swallowed throughout.
        let v = m.on_key(down(V, false), t);
        assert_eq!(v.decision, KeyDecision::Swallow);
        assert_eq!(
            v.emitted.expect("emitted").gesture.as_str(),
            "kb:v+verbatim"
        );
        assert_eq!(m.on_key(up(V, false), t).decision, KeyDecision::Swallow);
        assert_eq!(m.on_key(up(CAPS, false), t).decision, KeyDecision::Swallow);
    }

    #[test]
    fn control_verbatim_key_builds_normalized_chord() {
        let mut m = machine(DecisionConfig::default(), &["kb:control+v+verbatim"]);
        let t = Instant::now();
        // Control alone passes through and is tracked as a held modifier.
        assert_eq!(
            m.on_key(down(CONTROL, false), t).decision,
            KeyDecision::Pass
        );
        assert_eq!(
            m.on_key(down(CAPS, false), t).decision,
            KeyDecision::Swallow
        );
        let v = m.on_key(down(V, false), t);
        assert_eq!(v.decision, KeyDecision::Swallow);
        assert_eq!(
            v.emitted.expect("emitted").gesture.as_str(),
            "kb:control+v+verbatim"
        );
    }

    #[test]
    fn generalized_modifier_uses_generic_name() {
        // A right-control variant must generalize to `control`.
        let mut m = machine(DecisionConfig::default(), &["kb:control+v+verbatim"]);
        let t = Instant::now();
        assert_eq!(m.on_key(down(0xA3, false), t).decision, KeyDecision::Pass); // right control
        assert_eq!(
            m.on_key(down(CAPS, false), t).decision,
            KeyDecision::Swallow
        );
        let v = m.on_key(down(V, false), t);
        assert_eq!(
            v.emitted.expect("emitted").gesture.as_str(),
            "kb:control+v+verbatim"
        );
    }

    #[test]
    fn injected_events_are_treated_like_physical() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = Instant::now();
        let caps = KeyEvent {
            injected: true,
            ..down(CAPS, false)
        };
        let v = KeyEvent {
            injected: true,
            ..down(V, false)
        };
        assert_eq!(m.on_key(caps, t).decision, KeyDecision::Swallow);
        let decision = m.on_key(v, t);
        assert_eq!(decision.decision, KeyDecision::Swallow);
        assert!(decision.emitted.is_some());
    }

    #[test]
    fn trapped_key_up_swallowed_while_unrelated_key_up_passes() {
        let mut m = machine(DecisionConfig::default(), &[]);
        let t = Instant::now();

        // An ordinary keystroke with no modifier passes on the way down and up.
        assert_eq!(m.on_key(down(A, false), t).decision, KeyDecision::Pass);
        assert_eq!(m.on_key(up(A, false), t).decision, KeyDecision::Pass);

        // The modifier is trapped on the way down and again on the way up.
        assert_eq!(
            m.on_key(down(CAPS, false), t).decision,
            KeyDecision::Swallow
        );
        assert_eq!(m.on_key(up(CAPS, false), t).decision, KeyDecision::Swallow);
    }

    #[test]
    fn unknown_vk_passes_through_even_with_modifier_held() {
        let mut m = machine(DecisionConfig::default(), &[]);
        let t = Instant::now();
        assert_eq!(
            m.on_key(down(CAPS, false), t).decision,
            KeyDecision::Swallow
        );
        // 0x07 is undefined and unnamed; it passes through.
        assert_eq!(m.on_key(down(0x07, false), t).decision, KeyDecision::Pass);
    }

    #[test]
    fn solo_bound_gesture_fires_without_modifier() {
        // No Verbatim modifier held, but F6 is bound as a solo gesture.
        let mut m = machine(DecisionConfig::default(), &["kb:f6"]);
        let t = Instant::now();
        let f6 = m.on_key(down(0x75, false), t); // VK_F6
        assert_eq!(f6.decision, KeyDecision::Swallow);
        assert_eq!(f6.emitted.expect("emitted").gesture.as_str(), "kb:f6");
        assert_eq!(m.on_key(up(0x75, false), t).decision, KeyDecision::Swallow);
    }

    #[test]
    fn modifier_only_gesture_can_be_bound() {
        // Binding the modifier alone still emits while swallowing.
        let mut m = machine(DecisionConfig::default(), &["kb:verbatim"]);
        let t = Instant::now();
        let caps = m.on_key(down(CAPS, false), t);
        assert_eq!(caps.decision, KeyDecision::Swallow);
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
        let t = Instant::now();

        assert_eq!(m.on_key(down(CAPS, false), t).decision, KeyDecision::Pass);
        let z = m.on_key(down(Z, false), t);
        assert_eq!(z.decision, KeyDecision::Pass);
        assert!(z.emitted.is_none());
        assert_eq!(m.on_key(up(Z, false), t).decision, KeyDecision::Pass);

        let v = m.on_key(down(V, false), t);
        assert_eq!(v.decision, KeyDecision::Swallow);
        assert_eq!(
            v.emitted.expect("emitted").gesture.as_str(),
            "kb:v+verbatim"
        );
        assert_eq!(m.on_key(up(V, false), t).decision, KeyDecision::Swallow);

        assert_eq!(m.on_key(up(CAPS, false), t).decision, KeyDecision::Pass);
    }

    #[test]
    fn auto_repeat_of_modifier_does_not_break_chords() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = Instant::now();
        // Caps auto-repeats (repeated down with no up).
        assert_eq!(
            m.on_key(down(CAPS, false), t).decision,
            KeyDecision::Swallow
        );
        assert_eq!(
            m.on_key(down(CAPS, false), t).decision,
            KeyDecision::Swallow
        );
        let v = m.on_key(down(V, false), t);
        assert_eq!(v.decision, KeyDecision::Swallow);
        assert_eq!(
            v.emitted.expect("emitted").gesture.as_str(),
            "kb:v+verbatim"
        );
    }

    /// Presses and releases `kb:v+verbatim` once as a genuine (non-repeated)
    /// press at time `t`, returning the repeat count it carried.
    fn press_v_verbatim(m: &mut DecisionMachine, t: Instant) -> u8 {
        m.on_key(down(CAPS, false), t);
        let v = m.on_key(down(V, false), t);
        let repeat = v.emitted.expect("emitted").repeat;
        m.on_key(up(V, false), t);
        m.on_key(up(CAPS, false), t);
        repeat
    }

    #[test]
    fn first_press_carries_repeat_zero() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = Instant::now();
        assert_eq!(press_v_verbatim(&mut m, t), 0);
    }

    #[test]
    fn second_press_within_window_increments_repeat() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = Instant::now();
        assert_eq!(press_v_verbatim(&mut m, t), 0);
        let t2 = t + Duration::from_millis(100);
        assert_eq!(press_v_verbatim(&mut m, t2), 1);
    }

    #[test]
    fn third_press_within_window_increments_again() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = Instant::now();
        assert_eq!(press_v_verbatim(&mut m, t), 0);
        let t2 = t + Duration::from_millis(100);
        assert_eq!(press_v_verbatim(&mut m, t2), 1);
        let t3 = t2 + Duration::from_millis(100);
        assert_eq!(press_v_verbatim(&mut m, t3), 2);
    }

    #[test]
    fn press_after_window_elapses_resets_to_zero() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = Instant::now();
        assert_eq!(press_v_verbatim(&mut m, t), 0);
        let t2 = t + Duration::from_millis(100);
        assert_eq!(press_v_verbatim(&mut m, t2), 1);
        // Well past the 500 ms multi-press timeout since the second press.
        let t3 = t2 + Duration::from_millis(600);
        assert_eq!(press_v_verbatim(&mut m, t3), 0);
    }

    #[test]
    fn a_different_gesture_resets_the_streak() {
        let mut m = machine(
            DecisionConfig::default(),
            &["kb:v+verbatim", "kb:a+verbatim"],
        );
        let t = Instant::now();
        assert_eq!(press_v_verbatim(&mut m, t), 0);

        // Press a different bound gesture within the window.
        let t2 = t + Duration::from_millis(100);
        m.on_key(down(CAPS, false), t2);
        let a = m.on_key(down(A, false), t2);
        assert_eq!(a.emitted.expect("emitted").repeat, 0);
        m.on_key(up(A, false), t2);
        m.on_key(up(CAPS, false), t2);

        // verbatim+v again, still within the original window: the streak
        // was broken by the different gesture, so this is a fresh press.
        let t3 = t2 + Duration::from_millis(100);
        assert_eq!(press_v_verbatim(&mut m, t3), 0);
    }

    #[test]
    fn auto_repeat_does_not_advance_the_repeat_count() {
        // Holding the gesture's own key down auto-repeats its key-down with
        // no intervening key-up (unlike press_v_verbatim's up/down pairs,
        // which are genuine separate presses). NVDA does not treat
        // auto-repeat as a multi-press for script-repeat purposes, so every
        // auto-repeated emission must carry the same count as the press
        // that started the hold.
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let t = Instant::now();
        m.on_key(down(CAPS, false), t);

        let first = m.on_key(down(V, false), t);
        assert_eq!(first.emitted.expect("emitted").repeat, 0);

        // Auto-repeat: V goes down again with no key-up in between.
        let repeat1 = m.on_key(down(V, false), t);
        assert_eq!(repeat1.emitted.expect("emitted").repeat, 0);
        let repeat2 = m.on_key(down(V, false), t);
        assert_eq!(repeat2.emitted.expect("emitted").repeat, 0);

        m.on_key(up(V, false), t);
        m.on_key(up(CAPS, false), t);

        // A genuine second press afterward, within the window, still counts
        // as the second press (1), not a fourth (3): the auto-repeats above
        // never advanced the count.
        let t2 = t + Duration::from_millis(50);
        assert_eq!(press_v_verbatim(&mut m, t2), 1);
    }

    #[test]
    fn repeat_count_saturates_instead_of_overflowing() {
        let mut m = machine(DecisionConfig::default(), &["kb:v+verbatim"]);
        let mut t = Instant::now();
        let mut last = 0;
        for _ in 0..=300 {
            last = press_v_verbatim(&mut m, t);
            t += Duration::from_millis(10);
        }
        assert_eq!(last, u8::MAX);
    }
}
