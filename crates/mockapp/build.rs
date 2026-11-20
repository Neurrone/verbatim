//! Embeds mockapp's manifest, which declares Common Controls v6 for its
//! real tree view (`src/tree_view.rs`), through the MSVC linker, as
//! `verbatim-app`'s build script embeds Verbatim's; and compiles
//! `forms_menu.cs`, the Windows Forms window of the `remote_ops` test of a
//! Windows Forms menu item, naming it to the tests as `MOCKAPP_FORMS_MENU`.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let manifest = dir.join("mockapp.exe.manifest");
    println!("cargo:rerun-if-changed=mockapp.exe.manifest");
    if env::var("CARGO_CFG_TARGET_OS").expect("set by cargo") == "windows" {
        compile_forms_menu(&dir.join("forms_menu.cs"));
    }
    if env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default() != "msvc" {
        return;
    }
    // Resource 2, not 1: the process keeps the classic common controls,
    // which its edit control and the tests of it were written against, and
    // the tree view alone is created in an activation context made from
    // this manifest.
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED,ID=2");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );
}

/// Compiles `source` with the C# compiler of the .NET Framework 4.8.1 every
/// Windows 11 has, for the target's own architecture, so the window starts
/// in a few tenths of a second rather than after Windows PowerShell's
/// start, which on GitHub's runners took more than the test's wait.
fn compile_forms_menu(source: &Path) {
    println!("cargo:rerun-if-changed=forms_menu.cs");
    let framework = match env::var("CARGO_CFG_TARGET_ARCH")
        .expect("set by cargo")
        .as_str()
    {
        "x86_64" => "Framework64",
        "aarch64" => "FrameworkArm64",
        other => panic!("no .NET Framework directory is known for {other}"),
    };
    let csc = PathBuf::from(env::var("SystemRoot").expect("Windows sets SystemRoot"))
        .join("Microsoft.NET")
        .join(framework)
        .join("v4.0.30319")
        .join("csc.exe");
    let out = PathBuf::from(env::var("OUT_DIR").expect("set by cargo")).join("forms_menu.exe");
    let output = Command::new(&csc)
        .args([
            "/nologo",
            "/target:winexe",
            "/optimize+",
            "/reference:System.Windows.Forms.dll",
        ])
        .arg(format!("/out:{}", out.display()))
        .arg(source)
        .output()
        .unwrap_or_else(|error| panic!("{} could not start: {error}", csc.display()));
    assert!(
        output.status.success(),
        "{} failed with {}: {}{}",
        csc.display(),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    println!("cargo:rustc-env=MOCKAPP_FORMS_MENU={}", out.display());
}
