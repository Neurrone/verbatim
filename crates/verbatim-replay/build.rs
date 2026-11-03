//! Records the commit `verbatim-replay` is built from, so the replay can
//! say when it differs from the commit the dump's build was made from.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("set by cargo"));
    source_commit(&manifest_dir);
}

/// Sets `VERBATIM_COMMIT` to the commit this build is made from (`git
/// rev-parse HEAD`) and `VERBATIM_UNCOMMITTED_CHANGES` to whether its
/// working tree has uncommitted changes (`git status --porcelain` prints
/// anything), or to `unknown` and `false` when git cannot answer, as in a
/// source archive. Reruns when HEAD, the ref it points to, the packed refs
/// or the index change, so a commit does not leave a stale hash.
fn source_commit(manifest_dir: &Path) {
    let git = |args: &[&str]| -> Option<String> {
        let output = Command::new("git")
            .args(args)
            .current_dir(manifest_dir)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        Some(String::from_utf8(output.stdout).ok()?.trim_end().to_owned())
    };
    let commit = git(&["rev-parse", "HEAD"]);
    let status = git(&["--no-optional-locks", "status", "--porcelain"]);
    let (commit, uncommitted_changes) = match (commit, status) {
        (Some(commit), Some(status)) => (commit, !status.is_empty()),
        _ => ("unknown".to_owned(), false),
    };
    println!("cargo:rustc-env=VERBATIM_COMMIT={commit}");
    println!("cargo:rustc-env=VERBATIM_UNCOMMITTED_CHANGES={uncommitted_changes}");

    let mut watched = vec![
        "HEAD".to_owned(),
        "index".to_owned(),
        "packed-refs".to_owned(),
    ];
    watched.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    for name in watched {
        // A path that does not exist would rerun this script on every build.
        if let Some(path) = git(&["rev-parse", "--git-path", &name])
            .map(|path| manifest_dir.join(path))
            .filter(|path| path.exists())
        {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}
