//! Records the commit `verbatim-replay` is built from, so the replay can
//! say when it differs from the commit the dump's build was made from.

use std::env;
use std::path::PathBuf;

#[path = "../../build-support/source_commit.rs"]
mod source_commit;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    source_commit::source_commit(&manifest_dir);
}
