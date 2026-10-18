"""Puts e58 into a worktree: the mockapp logging patch and the test hooks.
usage: python apply.py <worktree> <e58 folder>"""
import shutil
import sys

wt, e58 = sys.argv[1], sys.argv[2]


def edit(path, old, new):
    p = f'{wt}/{path}'
    s = open(p, encoding='utf-8').read()
    assert s.count(old) == 1, (path, old)
    s = s.replace(old, new)
    open(p, 'w', encoding='utf-8', newline='\n').write(s)


shutil.copy(f'{e58}/e58_log.rs', f'{wt}/crates/mockapp/src/e58_log.rs')
edit('crates/mockapp/src/main.rs', 'mod held;', 'mod e58_log;\nmod held;')
edit('crates/mockapp/src/hits.rs', '''/// The live registrations per event, by event id.''', '''/// e58: whether mockapp logs what reaches its window thread, from
/// `release` on.
pub(crate) static LOG: AtomicBool = AtomicBool::new(false);

/// e58: writes `log <what> <unix us>` while logging.
pub(crate) fn log(what: &str) {
    if LOG.load(Ordering::SeqCst) {
        log_always(what);
    }
}

/// e58: writes `log <what> <unix us>`.
pub(crate) fn log_always(what: &str) {
    let us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "log {what} {us}");
    let _ = out.flush();
}

/// The live registrations per event, by event id.''')
edit('crates/mockapp/src/hits.rs', '''pub(crate) fn hit(method: Method) {
''', '''pub(crate) fn hit(method: Method) {
    log(&format!("call {method:?}"));
''')
edit('crates/mockapp/src/hits.rs', '''pub(crate) fn advise(event: i32, added: bool) {
''', '''pub(crate) fn advise(event: i32, added: bool) {
    log_always(&format!(
        "advise {} {event}",
        if added { "added" } else { "removed" }
    ));
''')
edit('crates/mockapp/src/held.rs', '''    let _out = std::io::stdout().lock();
    crate::hits::release();''', '''    let _out = std::io::stdout().lock();
    if std::env::var_os("E58_LOG").is_some() {
        crate::hits::LOG.store(true, Ordering::SeqCst);
    }
    crate::hits::release();''')
edit('crates/mockapp/src/window.rs', '''    let mut msg = MSG::default();
    // A standard Win32 message loop;''', '''    crate::e58_log::install();
    let mut msg = MSG::default();
    // A standard Win32 message loop;''')
edit('crates/mockapp/tests/common/mod.rs', '''    /// The next line mockapp prints, waiting at most `timeout`.''', '''    /// e58: the next line mockapp prints, if one comes within `timeout`.
    pub fn line_within(&self, timeout: Duration) -> Option<String> {
        self.lines.recv_timeout(timeout).ok()
    }

    /// The next line mockapp prints, waiting at most `timeout`.''')
shutil.copy(f'{e58}/e58.rs', f'{wt}/crates/mockapp/tests/uia_bugs/e58.rs')
edit('crates/mockapp/tests/uia_bugs/main.rs', 'mod support;', 'mod e58;\nmod support;')
edit('crates/mockapp/tests/uia_bugs/main.rs', '''    if std::env::var(setup_race::RACER).is_ok() {''', '''    if let Ok(window) = std::env::var(e58::LISTENER) {
        e58::listener(&window);
    }
    if std::env::var(setup_race::RACER).is_ok() {''')
edit('crates/mockapp/tests/uia_bugs/main.rs', '''    harness::run_isolated(&[
''', '''    harness::run_isolated(&[
        ("e58_trial", e58::e58_trial),
''')
