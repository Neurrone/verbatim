//! Builds the static wxWidgets libraries for the GUI's C++ layer (see
//! `build/wx.rs`).

#[path = "build/wx.rs"]
mod wx;

fn main() {
    println!("cargo:rerun-if-changed=build/wx.rs");
    let _wx = wx::prepare();
}
