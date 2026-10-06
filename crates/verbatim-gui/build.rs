//! Builds the GUI's C++ layer: static wxWidgets (`build/wx.rs`), then the
//! cxx bridge (`src/bridge.rs`) and `cpp/gui.cpp` compiled against it, and
//! the link lines for both.

#[path = "build/wx.rs"]
mod wx;

/// The Windows libraries wxWidgets' base and core libraries call into.
const SYSTEM_LIBRARIES: &[&str] = &[
    "kernel32", "user32", "gdi32", "gdiplus", "msimg32", "comdlg32", "winspool", "winmm",
    "shell32", "shlwapi", "comctl32", "ole32", "oleaut32", "uuid", "rpcrt4", "advapi32", "version",
    "ws2_32", "wininet", "oleacc", "uxtheme", "imm32",
];

fn main() {
    for watched in ["build/wx.rs", "src/bridge.rs", "cpp/gui.h", "cpp/gui.cpp"] {
        println!("cargo:rerun-if-changed={watched}");
    }
    let wx = wx::prepare();

    cxx_build::bridge("src/bridge.rs")
        .file("cpp/gui.cpp")
        .std("c++17")
        // `wx/setup.h` for this build comes first, ahead of the generic one
        // among the public headers.
        .include(&wx.setup)
        .include(&wx.include)
        .define("__WXMSW__", None)
        .define("UNICODE", None)
        .define("_UNICODE", None)
        // The libraries are release builds; this keeps the headers' view of
        // the build the same.
        .define("NDEBUG", None)
        .flag("/utf-8")
        // Standard C++ exceptions, as the libraries were built with. Without
        // it `_CPPUNWIND` is undefined, the headers turn wxUSE_EXCEPTIONS
        // off, and wxApp's exception virtuals vanish from this side's view
        // of its vtable: the library's first virtual call through the
        // application object (CreateTraits, in wxEntryStart) then lands on
        // the wrong function and crashes.
        .flag("/EHsc")
        .compile("verbatim-gui-cpp");

    println!("cargo:rustc-link-search=native={}", wx.lib.display());
    for library in &wx.libraries {
        println!("cargo:rustc-link-lib=static={library}");
    }
    for library in SYSTEM_LIBRARIES {
        println!("cargo:rustc-link-lib=dylib={library}");
    }
}
