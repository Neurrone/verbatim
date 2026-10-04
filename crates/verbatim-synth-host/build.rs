//! Embeds the host's application manifest, which makes UTF-8 the process's
//! code page. eSpeak NG opens its data with the C runtime's narrow file
//! functions, which take the path in the process code page; with UTF-8,
//! an install folder with any name (C:\Users\Zoë) still works.

fn main() {
    let manifest =
        std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo"))
            .join("verbatim-synth-host.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
