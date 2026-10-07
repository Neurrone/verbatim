//! The text a key types: the source of `Input::CharacterTyped`
//! (`docs/crates/verbatim-input-windows.md`, "Typed text").
//!
//! The hook translates each key it passes to the application with
//! `ToUnicodeEx`, using the foreground thread's keyboard layout, before the
//! application sees the key. Three things make that correct:
//!
//! - It never changes the keyboard state. Windows keeps a pending dead key
//!   (the accent of a US-International key, waiting for the letter it
//!   modifies) in state that every translation with that layout shares,
//!   measured on 2026-10-06 with one process translating a dead key and
//!   another process then translating a letter, which came back accented.
//!   A translation without `ToUnicodeEx`'s "do not change keyboard state"
//!   flag would consume the application's dead key, the classic fault of
//!   keyboard hooks. With the flag, the dead key's own press translates to
//!   nothing (and is not echoed), and the letter after it translates to the
//!   composed character, read from the pending state the application's own
//!   translation of the dead key left, without clearing it.
//! - It uses the layout of the thread the key goes to, not the hook
//!   thread's: each thread has its own layout.
//! - An input method composes keys into text the hook cannot know (Chinese,
//!   Japanese, and Korean input), so with an input method's layout active
//!   nothing is translated. Its committed text is not echoed; announcing
//!   compositions is a later milestone (`phase6-design.md`,
//!   "Internationalization in the text model"), and NVDA reads them from
//!   inside the application, which Verbatim's injection helper (decision
//!   D2) will do.
//! - A keyboard text service outside those languages (Vietnamese Telex and
//!   VNI, the Indic Phonetic keyboards) composes characters from several
//!   keys under the language's ordinary layout, whose handle cannot tell
//!   it apart. The active keyboard profile can: when it is a text service
//!   for the foreground layout's language, nothing is translated, so
//!   Verbatim stays silent rather than echoing the raw keys (decided
//!   2026-10-08). Echoing the composed text needs the injection helper and
//!   is on the roadmap for M6 (`docs/roadmap.md`, "Typed-character echo of
//!   composed text").
//!
//! Keys typed with `KEYEVENTF_UNICODE` (an on-screen keyboard, the
//! end-to-end harness) arrive as `VK_PACKET` carrying the UTF-16 unit
//! itself, a surrogate pair as two keys.

use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetKeyState, GetKeyboardLayout, HKL, ToUnicodeEx, VK_CAPITAL, VK_CONTROL,
    VK_LCONTROL, VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_PACKET, VK_RCONTROL, VK_RMENU,
    VK_RSHIFT, VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::TextServices::{
    CLSID_TF_InputProcessorProfiles, GUID_TFCAT_TIP_KEYBOARD, ITfInputProcessorProfileMgr,
    TF_INPUTPROCESSORPROFILE, TF_PROFILETYPE_INPUTPROCESSOR,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// `ToUnicodeEx`'s flag that leaves the keyboard state, a pending dead key
/// above all, unchanged (Windows 10 version 1607 and later).
const DO_NOT_CHANGE_STATE: u32 = 0x4;

/// The primary language ids whose layouts are input methods: Chinese,
/// Japanese, and Korean.
const INPUT_METHOD_LANGUAGES: [u16; 3] = [0x04, 0x11, 0x12];

/// Translates passed keys into the text they type, remembering the first
/// half of a surrogate pair typed as two `VK_PACKET` keys.
#[derive(Default)]
pub(crate) struct Typing {
    high_surrogate: Option<u16>,
    /// The text services' profile manager, which says whether the active
    /// keyboard profile is a text service; `None` when it could not be
    /// made, and then no layout is taken for a text service.
    profiles: Option<ITfInputProcessorProfileMgr>,
}

impl Typing {
    /// A translator that also asks the text services which keyboard
    /// profile is active. COM must be initialized on the calling thread,
    /// the thread that translates.
    pub(crate) fn with_text_services() -> Self {
        // SAFETY: the class id and interface are the text services' own,
        // and the caller initialized COM on this thread.
        let profiles = unsafe {
            CoCreateInstance::<_, ITfInputProcessorProfileMgr>(
                &CLSID_TF_InputProcessorProfiles,
                None,
                CLSCTX_INPROC_SERVER,
            )
        };
        if let Err(error) = &profiles {
            tracing::warn!(%error, "the text services' profile manager could not be made");
        }
        Self {
            high_surrogate: None,
            profiles: profiles.ok(),
        }
    }

    /// The active keyboard profile's type and language, or `None` when it
    /// cannot be read. Windows switches input methods for every application
    /// together unless the user chose otherwise, so this thread's active
    /// profile is the foreground application's; [`is_text_service`]
    /// requires its language to be the foreground layout's as well.
    fn active_keyboard_profile(&self) -> Option<(u32, u16)> {
        let profiles = self.profiles.as_ref()?;
        let mut profile = TF_INPUTPROCESSORPROFILE::default();
        // SAFETY: the category is a constant and the profile a local the
        // call fills.
        unsafe { profiles.GetActiveProfile(&GUID_TFCAT_TIP_KEYBOARD, &raw mut profile) }.ok()?;
        Some((profile.dwProfileType, profile.langid))
    }

    /// The text a key-down passed to the application types, or `None`.
    pub(crate) fn translate(&mut self, vk: u16, scan_code: u32) -> Option<String> {
        if vk == VK_PACKET.0 {
            return self.packet(u16::try_from(scan_code).ok()?);
        }
        self.high_surrogate = None;
        let modifiers = Modifiers::now();
        if !modifiers.allow_typing() {
            return None;
        }
        let layout = foreground_layout();
        if is_input_method(layout) || is_text_service(self.active_keyboard_profile(), layout) {
            return None;
        }
        let state = modifiers.key_state();
        let mut buffer = [0u16; 8];
        // SAFETY: the key state and the output buffer are locals of the
        // sizes the call takes; the flag keeps the shared keyboard state,
        // dead keys included, unchanged, so nothing the application relies
        // on is touched.
        let length = unsafe {
            ToUnicodeEx(
                u32::from(vk),
                scan_code,
                &state,
                &mut buffer,
                DO_NOT_CHANGE_STATE,
                Some(layout),
            )
        };
        // Negative for a dead key, which types nothing yet.
        let length = usize::try_from(length).ok()?;
        typed_text(&buffer[..length.min(buffer.len())])
    }

    /// The text of one `VK_PACKET` key's UTF-16 unit.
    fn packet(&mut self, unit: u16) -> Option<String> {
        if (0xD800..0xDC00).contains(&unit) {
            self.high_surrogate = Some(unit);
            return None;
        }
        let units: Vec<u16> = self
            .high_surrogate
            .take()
            .into_iter()
            .chain([unit])
            .collect();
        typed_text(&units)
    }
}

/// The text a key's UTF-16 output types: `None` for nothing, an unpaired
/// surrogate, or a control character other than a tab or a carriage return
/// (Enter), which edit text rather than type it.
pub(crate) fn typed_text(units: &[u16]) -> Option<String> {
    let text = String::from_utf16(units).ok()?;
    let typed = !text.is_empty()
        && text
            .chars()
            .all(|character| !character.is_control() || matches!(character, '\t' | '\r'));
    typed.then_some(text)
}

/// The modifier keys held as a key goes down.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent keys, each down or not"
)]
pub(crate) struct Modifiers {
    pub(crate) shift: bool,
    pub(crate) control: bool,
    pub(crate) alt: bool,
    pub(crate) windows: bool,
    pub(crate) caps_lock: bool,
}

impl Modifiers {
    /// The modifiers as they are now: the keys already down, read from the
    /// asynchronous key state, which a low-level hook sees updated for every
    /// key before the one it is deciding; Caps Lock's toggle from the key
    /// state.
    fn now() -> Self {
        let down = |vk: u16| {
            // SAFETY: GetAsyncKeyState takes any virtual-key code.
            let state = unsafe { GetAsyncKeyState(i32::from(vk)) };
            state < 0
        };
        Self {
            shift: down(VK_SHIFT.0),
            control: down(VK_CONTROL.0),
            alt: down(VK_MENU.0),
            windows: down(VK_LWIN.0) || down(VK_RWIN.0),
            // SAFETY: GetKeyState takes any virtual-key code.
            caps_lock: unsafe { GetKeyState(i32::from(VK_CAPITAL.0)) } & 1 != 0,
        }
    }

    /// Whether a key pressed with these modifiers types: not with Control or
    /// Alt alone (shortcuts and menu keys), nor with the Windows key; with
    /// both together it is `AltGr`, which types.
    pub(crate) fn allow_typing(self) -> bool {
        !self.windows && self.control == self.alt
    }

    /// The key state array `ToUnicodeEx` reads the modifiers from.
    fn key_state(self) -> [u8; 256] {
        let mut state = [0u8; 256];
        let mut set = |vks: &[u16], down: bool| {
            for &vk in vks {
                state[usize::from(vk)] = if down { 0x80 } else { 0 };
            }
        };
        set(&[VK_SHIFT.0, VK_LSHIFT.0, VK_RSHIFT.0], self.shift);
        set(&[VK_CONTROL.0, VK_LCONTROL.0, VK_RCONTROL.0], self.control);
        set(&[VK_MENU.0, VK_LMENU.0, VK_RMENU.0], self.alt);
        if self.caps_lock {
            state[usize::from(VK_CAPITAL.0)] = 0x01;
        }
        state
    }
}

/// The keyboard layout of the thread that owns the foreground window, the
/// thread a key goes to.
fn foreground_layout() -> HKL {
    // A null window or a thread id of zero yields the calling thread's
    // layout, a harmless fallback.
    // SAFETY: a local read with no preconditions.
    let foreground = unsafe { GetForegroundWindow() };
    // SAFETY: tolerates any handle, answering 0 for a null one.
    let thread = unsafe { GetWindowThreadProcessId(foreground, None) };
    // SAFETY: a local read of a thread's layout.
    unsafe { GetKeyboardLayout(thread) }
}

/// Whether `layout` is an input method's, whose keys compose text the hook
/// cannot know: one for Chinese, Japanese, or Korean, whose input methods
/// are text services under the language's own layout, or an older input
/// method's layout, whose handle's high word starts with hexadecimal E.
/// `ImmIsIME` cannot tell: with text services it answers true for every
/// layout, the US one included (found 2026-10-06).
fn is_input_method(layout: HKL) -> bool {
    let handle = layout.0 as usize;
    // The low word of a layout handle is its language id, whose low ten bits
    // are the primary language.
    let language = u16::try_from(handle & 0x3FF).unwrap_or(0);
    let device = (handle >> 16) & 0xFFFF;
    INPUT_METHOD_LANGUAGES.contains(&language) || device & 0xF000 == 0xE000
}

/// Whether the active keyboard `profile` (its type and language) is a
/// keyboard text service for `layout`'s language, which composes text the
/// hook cannot know: Vietnamese Telex and VNI, or an Indic Phonetic
/// keyboard, each under its language's ordinary layout.
fn is_text_service(profile: Option<(u32, u16)>, layout: HKL) -> bool {
    let language = u16::try_from(layout.0 as usize & 0xFFFF).unwrap_or(0);
    profile
        .is_some_and(|(kind, langid)| kind == TF_PROFILETYPE_INPUTPROCESSOR && langid == language)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn printable_text_a_tab_and_enter_are_typed() {
        assert_eq!(typed_text(&units("a")).as_deref(), Some("a"));
        assert_eq!(typed_text(&units("é")).as_deref(), Some("é"));
        assert_eq!(typed_text(&units("\t")).as_deref(), Some("\t"));
        assert_eq!(typed_text(&units("\r")).as_deref(), Some("\r"));
        assert_eq!(
            typed_text(&units("´x")).as_deref(),
            Some("´x"),
            "a dead key that does not compose types both"
        );
    }

    #[test]
    fn control_characters_and_nothing_are_not_typed() {
        assert_eq!(typed_text(&units("\u{8}")), None, "Backspace");
        assert_eq!(typed_text(&units("\u{1b}")), None, "Escape");
        assert_eq!(typed_text(&units("\u{1}")), None, "Control+A");
        assert_eq!(typed_text(&[]), None);
        assert_eq!(typed_text(&[0xD800]), None, "an unpaired surrogate");
    }

    #[test]
    fn input_method_layouts_are_known_by_language_or_handle() {
        let layout = |handle: usize| HKL(handle as *mut core::ffi::c_void);
        assert!(!is_input_method(layout(0x0409_0409)), "US English");
        assert!(!is_input_method(layout(0xF020_0409)), "US Dvorak");
        assert!(is_input_method(layout(0x0411_0411)), "Japanese");
        assert!(is_input_method(layout(0x0804_0804)), "Chinese");
        assert!(is_input_method(layout(0x0412_0412)), "Korean");
        assert!(
            is_input_method(layout(0xE001_0409)),
            "an older input method"
        );
    }

    #[test]
    fn a_text_service_for_the_layouts_language_is_known() {
        let vietnamese = HKL(0x042A_042A as *mut core::ffi::c_void);
        // Vietnamese Telex: a text service under the Vietnamese layout.
        assert!(is_text_service(
            Some((TF_PROFILETYPE_INPUTPROCESSOR, 0x042A)),
            vietnamese
        ));
        // The Vietnamese keyboard layout itself.
        assert!(!is_text_service(Some((2, 0x042A)), vietnamese));
        // A text service for another language than the foreground's.
        assert!(!is_text_service(
            Some((TF_PROFILETYPE_INPUTPROCESSOR, 0x0439)),
            vietnamese
        ));
        assert!(!is_text_service(None, vietnamese));
    }

    #[test]
    fn a_surrogate_pair_typed_as_two_packets_is_one_character() {
        let mut typing = Typing::default();
        let pair = units("😀");
        assert_eq!(typing.packet(pair[0]), None);
        assert_eq!(typing.packet(pair[1]).as_deref(), Some("😀"));
        assert_eq!(typing.packet(u16::from(b'a')).as_deref(), Some("a"));
    }

    #[test]
    fn shortcuts_do_not_type_but_altgr_does() {
        let plain = Modifiers::default();
        assert!(plain.allow_typing());
        assert!(
            Modifiers {
                shift: true,
                ..plain
            }
            .allow_typing()
        );
        assert!(
            !Modifiers {
                control: true,
                ..plain
            }
            .allow_typing()
        );
        assert!(!Modifiers { alt: true, ..plain }.allow_typing());
        assert!(
            Modifiers {
                control: true,
                alt: true,
                ..plain
            }
            .allow_typing(),
            "AltGr"
        );
        assert!(
            !Modifiers {
                windows: true,
                ..plain
            }
            .allow_typing()
        );
    }
}
