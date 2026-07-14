//! Wraps `vm/scripts/Build-VerbatimWindows11Image.ps1` (the existing,
//! already-working Packer wrapper — see `vm/README.md`) and locates the
//! `.vmcx` it exports, so `xtask vm create` can hand it to `Import-VM`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::VmResult;

/// Runs the Packer build wrapper with `-Force` from the repository root.
///
/// # Errors
///
/// Returns an error if the wrapper script is missing, cannot be launched,
/// or exits with a failure status.
pub(crate) fn build_image(repo_root: &Path) -> VmResult<()> {
    let script = repo_root
        .join("vm")
        .join("scripts")
        .join("Build-VerbatimWindows11Image.ps1");
    if !script.is_file() {
        return Err(format!(
            "Packer build wrapper not found at {}",
            script.display()
        ));
    }

    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .arg("-Force")
        .current_dir(repo_root)
        .status()
        .map_err(|error| format!("failed to launch {}: {error}", script.display()))?;
    if !status.success() {
        return Err(format!("Packer build failed: {status}"));
    }
    Ok(())
}

/// Finds the single exported `.vmcx` under the Packer output directory's
/// `Virtual Machines` folder.
///
/// # Errors
///
/// Returns an error if the output directory cannot be read, or does not
/// contain exactly one `.vmcx` file.
pub(crate) fn locate_exported_vmcx(repo_root: &Path) -> VmResult<PathBuf> {
    let output_dir = resolve_output_directory(repo_root);
    let vm_dir = output_dir.join("Virtual Machines");
    let entries = fs::read_dir(&vm_dir)
        .map_err(|error| format!("could not read {}: {error}", vm_dir.display()))?;

    let mut vmcx_files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("vmcx"))
        })
        .collect();

    match vmcx_files.len() {
        0 => Err(format!("no .vmcx file found under {}", vm_dir.display())),
        1 => Ok(vmcx_files.remove(0)),
        count => Err(format!(
            "expected exactly one .vmcx file under {}, found {count}",
            vm_dir.display()
        )),
    }
}

/// Resolves the Packer output directory the same way
/// `Build-VerbatimWindows11Image.ps1` does: `output_directory` from
/// `vm/local.pkrvars.hcl` if present, otherwise the template's own default.
fn resolve_output_directory(repo_root: &Path) -> PathBuf {
    let var_file = repo_root.join("vm").join("local.pkrvars.hcl");
    let relative = read_hcl_string(&var_file, "output_directory")
        .unwrap_or_else(|| "artifacts/packer/windows11".to_owned());
    repo_root.join(relative)
}

/// A minimal `name = "value"` line reader for `.pkrvars.hcl` files: good
/// enough for the flat variable files this harness uses, not a general HCL
/// parser. Mirrors `Get-HclStringValue` in
/// `Build-VerbatimWindows11Image.ps1`.
fn read_hcl_string(path: &Path, name: &str) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(name) else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim();
        if let Some(value) = rest.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            return Some(value.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_hcl_string_finds_a_matching_line() {
        let dir = std::env::temp_dir().join(format!(
            "xtask-vm-packer-build-test-{:?}",
            std::thread::current().id()
        ));
        fs::create_dir_all(&dir).expect("creates a temp dir");
        let path = dir.join("vars.hcl");
        fs::write(
            &path,
            "iso_path = \"iso/foo.iso\"\noutput_directory = \"artifacts/x\"\n",
        )
        .expect("writes the temp var file");

        assert_eq!(
            read_hcl_string(&path, "output_directory"),
            Some("artifacts/x".to_owned())
        );
        assert_eq!(read_hcl_string(&path, "temp_path"), None);

        fs::remove_dir_all(&dir).ok();
    }
}
