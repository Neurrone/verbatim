//! Embeds mockapp's manifest, which declares Common Controls v6 for its
//! real tree view (`src/tree_view.rs`), through the MSVC linker, as
//! `verbatim-app`'s build script embeds Verbatim's; compiles
//! `forms_menu.cs`, the Windows Forms window of the `remote_ops` test of a
//! Windows Forms menu item, naming it to the tests as `MOCKAPP_FORMS_MENU`;
//! and compiles the end-to-end suite's Windows Forms and WPF windows,
//! `text_box_form.cs`, `forms_password.cs` and `wpf_password.cs`, next to
//! the executables, where the suite's staging copies them from as it copies
//! `mockapp.exe`.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../../build-support/artifact_dir.rs"]
mod artifact_dir;

/// The references of a Windows Forms program.
const FORMS: &[&str] = &["System.Windows.Forms.dll", "System.Drawing.dll"];

/// The references of a WPF program, in the framework's `WPF` folder but for
/// `System.Xaml.dll`.
const WPF: &[&str] = &[
    r"WPF\PresentationFramework.dll",
    r"WPF\PresentationCore.dll",
    r"WPF\WindowsBase.dll",
    "System.Xaml.dll",
];

fn main() {
    let dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let manifest = dir.join("mockapp.exe.manifest");
    println!("cargo:rerun-if-changed=mockapp.exe.manifest");
    if env::var("CARGO_CFG_TARGET_OS").expect("set by cargo") == "windows" {
        let out = PathBuf::from(env::var("OUT_DIR").expect("set by cargo"));
        let forms_menu = out.join("forms_menu.exe");
        compile(&dir, "forms_menu", &forms_menu, FORMS);
        println!(
            "cargo:rustc-env=MOCKAPP_FORMS_MENU={}",
            forms_menu.display()
        );
        let executables = artifact_dir::artifact_dir();
        for (name, references) in [
            ("text_box_form", FORMS),
            ("forms_password", FORMS),
            ("wpf_password", WPF),
        ] {
            compile(
                &dir,
                name,
                &executables.join(format!("{name}.exe")),
                references,
            );
        }
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

/// Compiles `<name>.cs` in `dir` into the window program `out`, referencing
/// `references` in the framework's folder, with the C# compiler of the .NET
/// Framework 4.8.1 every Windows 11 has, for the target's own architecture,
/// so the window starts in a few tenths of a second rather than after
/// Windows PowerShell's start, which on GitHub's runners took more than the
/// tests' waits.
fn compile(dir: &Path, name: &str, out: &Path, references: &[&str]) {
    println!("cargo:rerun-if-changed={name}.cs");
    let framework = match env::var("CARGO_CFG_TARGET_ARCH")
        .expect("set by cargo")
        .as_str()
    {
        "x86_64" => "Framework64",
        "aarch64" => "FrameworkArm64",
        other => panic!("no .NET Framework directory is known for {other}"),
    };
    let framework = PathBuf::from(env::var("SystemRoot").expect("Windows sets SystemRoot"))
        .join("Microsoft.NET")
        .join(framework)
        .join("v4.0.30319");
    let csc = framework.join("csc.exe");
    let mut command = Command::new(&csc);
    command.args(["/nologo", "/target:winexe", "/optimize+"]);
    for reference in references {
        let path = framework.join(reference);
        assert!(path.is_file(), "{} is missing", path.display());
        command.arg(format!("/reference:{}", path.display()));
    }
    let output = command
        .arg(format!("/out:{}", out.display()))
        .arg(dir.join(format!("{name}.cs")))
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
}
