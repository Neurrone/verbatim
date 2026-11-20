//! The check of the committed NVDA captures, run as a unit test by `cargo
//! xtask ci`: every file under the scenarios' captures folder
//! (`crates/verbatim-e2e/captures`) and Core's
//! (`crates/verbatim-core/tests/fixtures/captures`) carries the header
//! both capture commands write, its pin is the `nvda` submodule's, and
//! something cites it; and every capture a source file cites exists
//! (`books/dev/src/how-to/nvda.md`, "Comparing a scenario"). A scenario
//! entry whose speech is `Capture` cites `<name>.txt` in the scenarios'
//! folder; any other capture is cited by its path in a source file.

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::plan;

    /// The folders that hold captures, relative to the workspace root.
    const FOLDERS: [&str; 2] = [
        "crates/verbatim-e2e/captures",
        "crates/verbatim-core/tests/fixtures/captures",
    ];

    /// The pin a capture's header names: from the text header's
    /// "Submodule pin: <commit> (<describe>)" line, or the JSON header's
    /// `"pin"` field, its commit alone.
    fn header_pin(text: &str) -> Option<String> {
        text.lines().find_map(|line| {
            let pin = if let Some(pin) = line.strip_prefix("Submodule pin: ") {
                pin.to_owned()
            } else {
                let value: serde_json::Value = serde_json::from_str(line).ok()?;
                if value["kind"] != "header" {
                    return None;
                }
                value["pin"].as_str()?.to_owned()
            };
            pin.split_whitespace().next().map(str::to_owned)
        })
    }

    /// Every capture path `sources` cite, as written in them.
    fn cited_paths(sources: &[String]) -> Vec<String> {
        let mut cited = Vec::new();
        for source in sources {
            for folder in FOLDERS {
                let mut rest = source.as_str();
                while let Some(at) = rest.find(folder) {
                    let path: String = rest[at..]
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || "/_.-".contains(*c))
                        .collect();
                    let path = path.trim_end_matches('.').to_owned();
                    if path.len() > folder.len() + 1 {
                        cited.push(path);
                    }
                    rest = &rest[at + folder.len()..];
                }
            }
        }
        cited.sort();
        cited.dedup();
        cited
    }

    /// What is wrong with the captures: `files` (each path relative to the
    /// root, with its text), given the submodule's commit `head`, the
    /// scenarios whose speech is `Capture`, the source files' texts, and
    /// whether a cited path exists.
    fn problems(
        files: &[(String, String)],
        head: &str,
        capture_scenarios: &[String],
        sources: &[String],
        exists: impl Fn(&str) -> bool,
    ) -> Vec<String> {
        let cited = cited_paths(sources);
        let mut problems = Vec::new();
        for (path, text) in files {
            match header_pin(text) {
                None => problems.push(format!("{path} has no header naming the nvda pin")),
                Some(pin) if pin.len() < 7 || !head.starts_with(&pin) => problems.push(format!(
                    "{path} was made at the pin {pin}, not the submodule's {head}: make it again"
                )),
                Some(_) => {}
            }
            let own = Path::new(path)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| {
                    path.starts_with(FOLDERS[0])
                        && capture_scenarios.iter().any(|name| name == stem)
                });
            if !own && !cited.contains(path) {
                problems.push(format!("{path} is cited by nothing"));
            }
        }
        for path in &cited {
            if !exists(path) {
                problems.push(format!("{path} is cited and does not exist"));
            }
        }
        problems
    }

    /// The commit the superproject's `HEAD` pins the submodule at `path`
    /// to, read from the gitlink itself (`git rev-parse HEAD:<path>`), so
    /// it does not need the submodule checked out: in a submodule not
    /// initialised, as on GitHub's runner, `git -C <path> rev-parse HEAD`
    /// answers the superproject's own commit.
    fn gitlink(root: &Path, path: &str) -> Result<String, String> {
        let output = std::process::Command::new("git")
            .args(["rev-parse", &format!("HEAD:{path}")])
            .current_dir(root)
            .output()
            .map_err(|error| format!("could not run git: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git rev-parse HEAD:{path} failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let commit = String::from_utf8(output.stdout)
            .map_err(|error| format!("git printed something not UTF-8: {error}"))?;
        Ok(commit.trim().to_owned())
    }

    #[test]
    fn the_pin_is_the_gitlink_not_the_superprojects_commit() {
        let root = root();
        let pin = gitlink(&root, "nvda").expect("reads the nvda gitlink");
        assert_eq!(pin.len(), 40, "{pin}");
        assert!(pin.chars().all(|c| c.is_ascii_hexdigit()), "{pin}");
        let superproject = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&root)
            .output()
            .expect("runs git");
        assert_ne!(
            String::from_utf8_lossy(&superproject.stdout).trim(),
            pin,
            "the superproject's commit is not the submodule's pin"
        );
        assert!(gitlink(&root, "no-such-submodule").is_err());
    }

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask sits in the workspace")
            .to_path_buf()
    }

    fn rust_sources(dir: &Path, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_sources(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                out.push(text);
            }
        }
    }

    #[test]
    fn every_capture_has_the_pins_header_and_a_citation_and_every_citation_a_file() {
        let root = root();
        let head = gitlink(&root, "nvda").expect("reads the nvda submodule's pin");
        let mut files = Vec::new();
        for folder in FOLDERS {
            let Ok(entries) = std::fs::read_dir(root.join(folder)) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let text = std::fs::read_to_string(entry.path()).expect("reads a capture");
                files.push((format!("{folder}/{name}"), text));
            }
        }
        let registry = std::fs::read_to_string(root.join("crates/verbatim-e2e/src/registry.rs"))
            .expect("reads the registry");
        let capture_scenarios: Vec<String> = plan::registry_entries(&registry)
            .expect("reads the registry's entries")
            .into_iter()
            .filter(|entry| entry.capture)
            .map(|entry| entry.name)
            .collect();
        let mut sources = Vec::new();
        rust_sources(&root.join("crates"), &mut sources);
        let found = problems(&files, &head, &capture_scenarios, &sources, |path| {
            root.join(path).exists()
        });
        assert!(found.is_empty(), "{}", found.join("\n"));
    }

    #[test]
    fn a_capture_without_a_header_at_a_moved_pin_uncited_or_missing_fails() {
        let head = "732d1ce2f0123456789abcdef0123456789abcde";
        let scenario = |text: &str| {
            (
                "crates/verbatim-e2e/captures/toast.txt".to_owned(),
                text.to_owned(),
            )
        };
        let core = |text: &str| {
            (
                "crates/verbatim-core/tests/fixtures/captures/race.txt".to_owned(),
                text.to_owned(),
            )
        };
        let header = "NVDA: alpha-58022,732d1ce2\nSubmodule pin: 732d1ce2f (release)\n";
        let json = "{\"kind\":\"header\",\"nvda\":\"alpha\",\"pin\":\"732d1ce2f (release)\"}\n";
        let toast = ["toast".to_owned()];
        let citing = ["// crates/verbatim-core/tests/fixtures/captures/race.txt.".to_owned()];
        let none = |_: &str| true;
        assert_eq!(
            problems(&[scenario(header), core(json)], head, &toast, &citing, none),
            Vec::<String>::new()
        );
        assert_eq!(
            problems(&[scenario("Step 1: start\n")], head, &toast, &[], none),
            ["crates/verbatim-e2e/captures/toast.txt has no header naming the nvda pin"]
        );
        assert_eq!(
            problems(&[scenario(header)], "ffffffff", &toast, &[], none),
            [
                "crates/verbatim-e2e/captures/toast.txt was made at the pin 732d1ce2f, not the submodule's ffffffff: make it again"
            ]
        );
        assert_eq!(
            problems(&[core(json)], head, &toast, &[], none),
            ["crates/verbatim-core/tests/fixtures/captures/race.txt is cited by nothing"]
        );
        assert_eq!(
            problems(&[], head, &toast, &citing, |_| false),
            ["crates/verbatim-core/tests/fixtures/captures/race.txt is cited and does not exist"]
        );
    }
}
