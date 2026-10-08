//! Reading whether a lock key is on, for
//! [`Request::KeyToggled`](crate::protocol::Request::KeyToggled): state a
//! test reads independently of the screen reader before it presses the
//! key, so the announcement it expects is fixed beforehand.

use std::io;

use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;

/// Whether the lock key `name` (a key name of the control plane's
/// vocabulary, such as `scrolllock`) is on.
///
/// A thread's view of the keyboard is taken from the system's when the
/// thread first reads it, and changes only with the input it handles
/// after that, so the read is made on a new thread, which has handled
/// none.
///
/// # Errors
///
/// Returns an error if `name` names no key.
pub fn toggled(name: &str) -> io::Result<bool> {
    let key = verbatim_input::keys::vk_from_name(name).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("no key named {name:?}"),
        )
    })?;
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                // SAFETY: takes any virtual-key code.
                let state = unsafe { GetKeyState(i32::from(key.vk)) };
                state & 1 != 0
            })
            .join()
            .map_err(|_| io::Error::other("the key-state thread panicked"))
    })
}
