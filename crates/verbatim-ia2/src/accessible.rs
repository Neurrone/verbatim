//! Safe wrappers over MSAA: [`Accessible`], an `IAccessible` with the child id
//! it addresses, and [`Related`], what an MSAA method that names another
//! object answers.
//!
//! The `IAccessible` methods and the `oleacc` acquisition functions are
//! `unsafe` in the `windows` bindings only because the bindings are
//! generated: given a live interface pointer, which the smart pointer
//! guarantees, and out-parameters owned by the caller, they cannot cause
//! undefined behavior. Each method here holds one such call with its
//! `SAFETY` comment, so the code above it (`acquire`) is safe code. The
//! `VARIANT` union reads stay here too, applied only to `VARIANT`s that
//! `oleacc` or an `IAccessible` method filled in, whose type tag matches
//! their contents.
//!
//! Every method that reaches the application's process counts itself as one
//! MSAA call ([`crate::calls`]), exactly once per underlying API call, so
//! a caller counts nothing itself.

use std::ffi::c_void;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoTaskMemFree, IDispatch};
use windows::Win32::System::Variant::{VARIANT, VT_DISPATCH, VT_EMPTY, VT_I4};
use windows::Win32::UI::Accessibility::{
    AccessibleChildren, AccessibleObjectFromEvent, AccessibleObjectFromWindow, IAccIdentity,
    IAccessible, WindowFromAccessibleObject,
};
use windows::Win32::UI::WindowsAndMessaging::OBJID_CLIENT;
use windows::core::{AgileReference, IUnknown, Interface};

use verbatim_model::{CallKind, Rect};

use crate::calls::count;
use crate::com::{CHILDID_SELF, child_variant};

/// The most children [`Accessible::children`] asks for in one call, far
/// more than any real container presents at once.
pub(crate) const MAX_CHILDREN: usize = 10_000;

/// An `IAccessible` and the child id it is read at: `CHILDID_SELF` for the
/// object itself, or a simple child addressed by id on it.
///
/// The methods named after an `IAccessible` property (`name`, `role`, and
/// so on) read it for this child id. The ones about the object as a whole,
/// [`parent`](Self::parent), [`child_count`](Self::child_count),
/// [`children`](Self::children), [`focus`](Self::focus),
/// [`selection`](Self::selection), [`window`](Self::window), and
/// [`identity_string`](Self::identity_string)'s interface, ignore the child
/// id, as MSAA does. Every method is a blocking cross-process call, worker
/// only, and counts itself.
#[derive(Clone, Debug)]
pub(crate) struct Accessible {
    /// The object.
    object: IAccessible,
    /// The child id the property reads address.
    child: i32,
}

impl Accessible {
    /// `object` read at `child`. Any child id is safe: one the object does
    /// not have makes its reads fail.
    pub(crate) fn new(object: IAccessible, child: i32) -> Self {
        Self { object, child }
    }

    /// The object named by a `WinEvent` address, at the child id
    /// `AccessibleObjectFromEvent` answers, or `None` if it cannot be
    /// acquired.
    ///
    /// `AccessibleObjectFromEvent` always answers the child as a `VT_I4`; any
    /// other form would be read as `CHILDID_SELF`.
    pub(crate) fn from_event(hwnd: isize, id_object: i32, id_child: i32) -> Option<Self> {
        let mut object: Option<IAccessible> = None;
        let mut child = VARIANT::default();
        count(CallKind::Msaa);
        // SAFETY: both out-parameters are locals this call initializes; a
        // stale or invalid address makes it fail rather than misbehave.
        unsafe {
            AccessibleObjectFromEvent(
                HWND(hwnd as *mut c_void),
                id_object.cast_unsigned(),
                id_child.cast_unsigned(),
                &raw mut object,
                &raw mut child,
            )
        }
        .ok()?;
        let child = match related(&child) {
            Related::Child(child) => child,
            _ => CHILDID_SELF,
        };
        Some(Self::new(object?, child))
    }

    /// The client object of the window `hwnd` (`OBJID_CLIENT`), at
    /// `CHILDID_SELF`, or `None` if it cannot be acquired.
    pub(crate) fn client_of_window(hwnd: isize) -> Option<Self> {
        let mut object: Option<IAccessible> = None;
        count(CallKind::Msaa);
        // SAFETY: the out-pointer is a local `Option<IAccessible>`, which is
        // null-pointer-optimized, so it has the layout of the interface
        // pointer the call writes for `IAccessible::IID`, or leaves null; an
        // invalid handle makes the call fail.
        unsafe {
            AccessibleObjectFromWindow(
                HWND(hwnd as *mut c_void),
                OBJID_CLIENT.0.cast_unsigned(),
                &IAccessible::IID,
                (&raw mut object).cast::<*mut c_void>(),
            )
        }
        .ok()?;
        object.map(|object| Self::new(object, CHILDID_SELF))
    }

    /// The same object at another child id.
    pub(crate) fn with_child(&self, child: i32) -> Self {
        Self::new(self.object.clone(), child)
    }

    /// The child id this reads at.
    pub(crate) fn child(&self) -> i32 {
        self.child
    }

    /// An agile reference to the object, for the registry to keep. Not a
    /// counted call.
    pub(crate) fn agile(&self) -> Option<AgileReference<IAccessible>> {
        AgileReference::new(&self.object).ok()
    }

    /// The address of the object's canonical `IUnknown`, which identifies
    /// the COM object while a reference to it is held. Not a counted call:
    /// COM answers `IUnknown` locally.
    pub(crate) fn canonical(&self) -> Option<usize> {
        self.object
            .cast::<IUnknown>()
            .ok()
            .map(|unknown| unknown.as_raw() as usize)
    }

    /// `accName`, or `None` when the read fails.
    pub(crate) fn name(&self) -> Option<String> {
        count(CallKind::Msaa);
        // SAFETY: a live interface and an integer child `VARIANT`.
        unsafe { self.object.get_accName(&child_variant(self.child)) }
            .ok()
            .map(|text| text.to_string())
    }

    /// `accValue`, or `None` when the read fails.
    pub(crate) fn value(&self) -> Option<String> {
        count(CallKind::Msaa);
        // SAFETY: a live interface and an integer child `VARIANT`.
        unsafe { self.object.get_accValue(&child_variant(self.child)) }
            .ok()
            .map(|text| text.to_string())
    }

    /// `accDescription`, or `None` when the read fails.
    pub(crate) fn description(&self) -> Option<String> {
        count(CallKind::Msaa);
        // SAFETY: a live interface and an integer child `VARIANT`.
        unsafe { self.object.get_accDescription(&child_variant(self.child)) }
            .ok()
            .map(|text| text.to_string())
    }

    /// `accKeyboardShortcut`, or `None` when the read fails.
    pub(crate) fn keyboard_shortcut(&self) -> Option<String> {
        count(CallKind::Msaa);
        // SAFETY: a live interface and an integer child `VARIANT`.
        unsafe {
            self.object
                .get_accKeyboardShortcut(&child_variant(self.child))
        }
        .ok()
        .map(|text| text.to_string())
    }

    /// `accDefaultAction`, the default action's name, or `None` when the
    /// read fails.
    pub(crate) fn default_action(&self) -> Option<String> {
        count(CallKind::Msaa);
        // SAFETY: a live interface and an integer child `VARIANT`.
        unsafe { self.object.get_accDefaultAction(&child_variant(self.child)) }
            .ok()
            .map(|text| text.to_string())
    }

    /// `accDoDefaultAction`.
    pub(crate) fn do_default_action(&self) -> windows::core::Result<()> {
        count(CallKind::Msaa);
        // SAFETY: a live interface and an integer child `VARIANT`.
        unsafe { self.object.accDoDefaultAction(&child_variant(self.child)) }
    }

    /// `accRole` as the raw MSAA role number, or `None` when the read fails
    /// or answers a string role.
    pub(crate) fn role(&self) -> Option<i32> {
        count(CallKind::Msaa);
        // SAFETY: a live interface and an integer child `VARIANT`.
        let role = unsafe { self.object.get_accRole(&child_variant(self.child)) }.ok()?;
        // A string role does not convert; the `VARIANT`'s own `Drop`
        // (`VariantClear`) frees it.
        i32::try_from(&role).ok()
    }

    /// `accState` as the raw MSAA state word, or `None` when the read fails.
    pub(crate) fn state(&self) -> Option<i32> {
        count(CallKind::Msaa);
        // SAFETY: a live interface and an integer child `VARIANT`.
        let state = unsafe { self.object.get_accState(&child_variant(self.child)) }.ok()?;
        i32::try_from(&state).ok()
    }

    /// `accLocation`, in screen coordinates, or `None` when the call fails.
    pub(crate) fn location(&self) -> Option<Rect> {
        let (mut left, mut top, mut width, mut height) = (0i32, 0i32, 0i32, 0i32);
        count(CallKind::Msaa);
        // SAFETY: a live interface, an integer child `VARIANT`, and four
        // local out-parameters.
        unsafe {
            self.object.accLocation(
                &raw mut left,
                &raw mut top,
                &raw mut width,
                &raw mut height,
                &child_variant(self.child),
            )
        }
        .ok()?;
        Some(Rect {
            left,
            top,
            width,
            height,
        })
    }

    /// `accParent` as an `IAccessible` at `CHILDID_SELF`: the error when
    /// `accParent` fails, and `None` when the parent it answers is not an
    /// `IAccessible`. Two calls when `accParent` succeeds, since asking the
    /// parent for `IAccessible` is a counted `QueryInterface`.
    pub(crate) fn parent(&self) -> windows::core::Result<Option<Self>> {
        count(CallKind::Msaa);
        // SAFETY: a live interface; the method takes no other arguments.
        let parent = unsafe { self.object.accParent() }?;
        Ok(cast_remote::<IAccessible>(&parent)
            .ok()
            .map(|parent| Self::new(parent, CHILDID_SELF)))
    }

    /// `accChildCount`.
    pub(crate) fn child_count(&self) -> windows::core::Result<i32> {
        count(CallKind::Msaa);
        // SAFETY: a live interface; the method takes no other arguments.
        unsafe { self.object.accChildCount() }
    }

    /// Up to `max` of the object's children from the first, never more
    /// than [`MAX_CHILDREN`], through `AccessibleChildren`, or `None` when
    /// the call fails.
    pub(crate) fn children(&self, max: usize) -> Option<Vec<Related>> {
        // `max` usually comes from the application's own `accChildCount`;
        // an absurd count must not size an allocation that aborts.
        let max = max.min(MAX_CHILDREN);
        let mut buffer: Vec<VARIANT> = (0..max).map(|_| VARIANT::default()).collect();
        let mut obtained = 0i32;
        count(CallKind::Msaa);
        // SAFETY: a live interface; `buffer` is a local slice of `max` empty
        // `VARIANT`s, which the call fills no further than its length, and
        // `obtained` a local it writes the count to.
        unsafe { AccessibleChildren(&self.object, 0, &mut buffer, &raw mut obtained) }.ok()?;
        let obtained = usize::try_from(obtained).unwrap_or(0).min(buffer.len());
        Some(buffer[..obtained].iter().map(related).collect())
    }

    /// `accNavigate` in the MSAA direction `navdir`, from this child id.
    pub(crate) fn navigate(&self, navdir: i32) -> windows::core::Result<Related> {
        count(CallKind::Msaa);
        // SAFETY: a live interface and an integer child `VARIANT`.
        let result = unsafe { self.object.accNavigate(navdir, &child_variant(self.child)) }?;
        Ok(related(&result))
    }

    /// `accFocus`: the child or object within this one that has the focus.
    pub(crate) fn focus(&self) -> windows::core::Result<Related> {
        count(CallKind::Msaa);
        // SAFETY: a live interface; the method takes no other arguments.
        let focus = unsafe { self.object.accFocus() }?;
        Ok(related(&focus))
    }

    /// `accSelection`: the selected child or object within this one.
    pub(crate) fn selection(&self) -> windows::core::Result<Related> {
        count(CallKind::Msaa);
        // SAFETY: a live interface; the method takes no other arguments.
        let selection = unsafe { self.object.accSelection() }?;
        Ok(related(&selection))
    }

    /// The window that owns the object (`WindowFromAccessibleObject`), or
    /// `None` when the call fails.
    pub(crate) fn window(&self) -> Option<isize> {
        let mut hwnd = HWND::default();
        count(CallKind::Msaa);
        // SAFETY: a live interface and a local out-parameter, read only on
        // success.
        unsafe { WindowFromAccessibleObject(&self.object, Some(&raw mut hwnd)) }.ok()?;
        Some(hwnd.0 as isize)
    }

    /// The object's MSAA identity string for this child id
    /// (`IAccIdentity::GetIdentityString`), or `None` when it offers none.
    /// Two calls when the object has the interface, since asking for it is a
    /// counted `QueryInterface`.
    pub(crate) fn identity_string(&self) -> Option<Vec<u8>> {
        let identity = cast_remote::<IAccIdentity>(&self.object).ok()?;
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut length = 0u32;
        count(CallKind::Msaa);
        // SAFETY: a live interface and two local out-parameters.
        unsafe {
            identity.GetIdentityString(self.child.cast_unsigned(), &raw mut data, &raw mut length)
        }
        .ok()?;
        if data.is_null() {
            return None;
        }
        let bytes = usize::try_from(length).ok().map(|length| {
            // SAFETY: on success `data` points to `length` bytes the call
            // allocated with the COM allocator, alive until freed below.
            unsafe { std::slice::from_raw_parts(data, length) }.to_vec()
        });
        // SAFETY: `data` came from the COM allocator and is freed once.
        unsafe { CoTaskMemFree(Some(data.cast_const().cast())) };
        bytes
    }
}

/// What an MSAA method that names another object answered (`accNavigate`,
/// `accFocus`, `accSelection`, and each entry of `AccessibleChildren`).
#[derive(Debug)]
pub(crate) enum Related {
    /// Nothing (`VT_EMPTY`).
    Nothing,
    /// A child by id on the object asked (`VT_I4`).
    Child(i32),
    /// An object of its own (`VT_DISPATCH`), `None` for a null pointer. It
    /// is asked for `IAccessible` only by [`Related::object`].
    Object(Option<IDispatch>),
    /// Any other form.
    Other,
}

impl Related {
    /// The object this names as an `IAccessible` at `CHILDID_SELF`, or
    /// `None` when it names no object, a null one, or one that is not an
    /// `IAccessible`. Asking for `IAccessible` is a counted
    /// `QueryInterface`, made only for a non-null object.
    pub(crate) fn object(&self) -> Option<Accessible> {
        let Self::Object(Some(dispatch)) = self else {
            return None;
        };
        cast_remote::<IAccessible>(dispatch)
            .ok()
            .map(|object| Accessible::new(object, CHILDID_SELF))
    }
}

/// `object` as interface `T`, by a `QueryInterface` counted as a
/// cross-process call: on an object from another process, COM answers it
/// locally only when its proxy already holds `T` (see [`crate::calls`]).
fn cast_remote<T: Interface>(object: &impl Interface) -> windows::core::Result<T> {
    count(CallKind::Msaa);
    object.cast::<T>()
}

/// Reads a `VARIANT` that `oleacc` or an `IAccessible` method filled in as a
/// [`Related`]. Private to this module, which applies it only to such
/// `VARIANT`s: their type tag always matches the union field they set.
fn related(value: &VARIANT) -> Related {
    let vt = value.vt();
    if vt == VT_EMPTY {
        Related::Nothing
    } else if vt == VT_I4 {
        // SAFETY: the tag says the union holds `lVal`.
        Related::Child(unsafe { value.Anonymous.Anonymous.Anonymous.lVal })
    } else if vt == VT_DISPATCH {
        // SAFETY: the tag says the union holds `pdispVal`, an interface
        // pointer or null; cloning it adds a reference, and the `VARIANT`'s
        // own reference is released by its `Drop` (`VariantClear`).
        Related::Object(unsafe { (*value.Anonymous.Anonymous.Anonymous.pdispVal).clone() })
    } else {
        Related::Other
    }
}
