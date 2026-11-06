//! Inside a remote operation, conhost's window has no handle or title
//! (`uia-facts/inside-a-remote-operation-conhosts-window-has-no-handle-or-title/readme.md`).
//!
//! - `parent`: a console host started directly (`conhost.exe cmd.exe`) on
//!   a desktop of the program's own; a client there finds its text area
//!   (the window element's descendant of the document control type) and
//!   reads the text area's raw-view parent's native window handle and name
//!   inside a remote operation, navigating to it there, and through the
//!   client side's raw view walker.

use uia_facts_support::args::{Args, trials};
use uia_facts_support::console::Console;
use uia_facts_support::report::{Report, deterministic, runs};
use uia_facts_support::role::{self, Role};
use uia_facts_support::{exit_now, rop, uia};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::{
    TreeScope_Descendants, UIA_ControlTypePropertyId, UIA_DocumentControlTypeId,
    UIA_NamePropertyId, UIA_NativeWindowHandlePropertyId,
};

/// The desktop the console host runs on.
const DESKTOP: &str = "uia-facts-conhost-operation";

fn main() {
    let args = Args::parse();
    uia::mta();
    if let Some((name, _)) = &args.role {
        assert_eq!(name, "console", "the one role is console");
        console_role();
    }
    let mut report =
        Report::start("inside-a-remote-operation-conhosts-window-has-no-handle-or-title");
    if args.wants("parent") {
        report.case("parent", "on a desktop of its own: a console host's text area; its parent's window handle and name read inside a remote operation and through the client side");
        parent(&mut report);
    }
    report.finish();
}

/// On this role's desktop: a console host, its text area's parent read
/// both ways; reports `inside <handle is 0> <name is empty>` and
/// `outside <handle is the window> <name is not empty>`.
fn console_role() -> ! {
    let console = Console::start();
    let client = uia::client();
    let window = uia::element(&client, console.hwnd).expect("the console window");
    let document = VARIANT::from(UIA_DocumentControlTypeId.0);
    // SAFETY: a live client object and a VARIANT holding an I4.
    let condition = unsafe { client.CreatePropertyCondition(UIA_ControlTypePropertyId, &document) }
        .expect("a condition");
    // SAFETY: live element and condition.
    let area = unsafe { window.FindFirst(TreeScope_Descendants, &condition) }
        .expect("the console's text area");
    let handle = rop::property(&area, UIA_NativeWindowHandlePropertyId.0, true)
        .unwrap_or_else(|e| uia::hresult(e.code()));
    let name =
        rop::property(&area, UIA_NamePropertyId.0, true).unwrap_or_else(|e| uia::hresult(e.code()));
    role::report(&format!("inside {} {}", handle == "0", name.is_empty()));
    uia_facts_support::note::evidence(&format!("inside: handle {handle}, name {name:?}"));
    // SAFETY: a live client object.
    let walker = unsafe { client.RawViewWalker() }.expect("the raw view walker");
    // SAFETY: a live walker and element.
    let up = unsafe { walker.GetParentElement(&area) }.expect("the parent");
    // SAFETY: a live element.
    let handle = unsafe { up.CurrentNativeWindowHandle() }.map_or(0, |h| h.0 as isize);
    let name = uia::name(&up).unwrap_or_default();
    role::report(&format!(
        "outside {} {}",
        handle == console.hwnd,
        !name.is_empty()
    ));
    drop(console);
    exit_now(0)
}

fn parent(report: &mut Report) {
    let mut inside = Vec::new();
    let mut outside = Vec::new();
    for _ in 0..trials() {
        let console = Role::start_on(Some(DESKTOP), "console", &[]);
        for line in [console.expect_line(), console.expect_line()] {
            let words: Vec<&str> = line.split(' ').collect();
            let pair = (words[1] == "true", words[2] == "true");
            match words[0] {
                "inside" => inside.push(pair),
                "outside" => outside.push(pair),
                other => panic!("the console role said {other}"),
            }
        }
    }
    report.claim(
        "inside a remote operation, the window element above the text area: (its handle is 0, its name is empty)",
        &runs(&inside),
        "no handle and no name",
        deterministic(&inside, &(true, true)),
    );
    report.claim(
        "through the client side's walker: (its handle is the console window's, it has a name)",
        &runs(&outside),
        "the window's handle and title",
        deterministic(&outside, &(true, true)),
    );
}
