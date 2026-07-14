//! A minimal `.env` reader for the guest credentials `xtask vm` needs to
//! reach the VM over PowerShell Direct: `VERBATIM_VM_USERNAME` and
//! `VERBATIM_VM_PASSWORD`, read from the repo-root `.env` file (gitignored,
//! never committed).
//!
//! Deliberately hand-rolled rather than pulling in a `dotenv` crate: the
//! format needed here is `KEY=VALUE` lines, comments, and blank lines, a
//! handful of lines of parsing that would not shrink meaningfully behind a
//! dependency.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use super::VmResult;

/// The guest's local administrator credentials, matching
/// `var.admin_username` / `var.admin_password` in `vm/variables.pkr.hcl`
/// and the Winlogon autologon `vm/scripts/Initialize-VerbatimHarness.ps1`
/// configures.
#[derive(Debug)]
pub(crate) struct GuestCredentials {
    pub(crate) username: String,
    pub(crate) password: String,
}

/// Reads `VERBATIM_VM_USERNAME` and `VERBATIM_VM_PASSWORD` from
/// `<repo_root>/.env`.
///
/// # Errors
///
/// Returns an error if `.env` cannot be read or either variable is absent.
pub(crate) fn load_guest_credentials(repo_root: &Path) -> VmResult<GuestCredentials> {
    let path = repo_root.join(".env");
    let text = fs::read_to_string(&path).map_err(|error| {
        format!(
            "could not read {} (guest credentials for the VM harness): {error}",
            path.display()
        )
    })?;
    let values = parse(&text);

    let username = values
        .get("VERBATIM_VM_USERNAME")
        .cloned()
        .ok_or_else(|| format!("{} has no VERBATIM_VM_USERNAME", path.display()))?;
    let password = values
        .get("VERBATIM_VM_PASSWORD")
        .cloned()
        .ok_or_else(|| format!("{} has no VERBATIM_VM_PASSWORD", path.display()))?;

    Ok(GuestCredentials { username, password })
}

fn parse(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, raw_value) = line.split_once('=')?;
            let key = key.trim().to_owned();
            let value = unquote(raw_value.trim());
            Some((key, value))
        })
        .collect()
}

/// Strips a single matching pair of surrounding quotes, the one bit of
/// dotenv convention worth honoring even though the repo's own `.env` never
/// needs it (plain `KEY=value`, no quoting).
fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        value[1..value.len() - 1].to_owned()
    } else {
        value.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_quoted_and_commented_lines() {
        let text = "\
# a comment
VERBATIM_VM_USERNAME=verbatim
VERBATIM_VM_PASSWORD=\"has spaces\"

TRAILING = value  ";
        let values = parse(text);
        assert_eq!(
            values.get("VERBATIM_VM_USERNAME"),
            Some(&"verbatim".to_owned())
        );
        assert_eq!(
            values.get("VERBATIM_VM_PASSWORD"),
            Some(&"has spaces".to_owned())
        );
        assert_eq!(values.get("TRAILING"), Some(&"value".to_owned()));
    }

    #[test]
    fn load_guest_credentials_reports_a_missing_file_clearly() {
        let missing_root = std::env::temp_dir().join("xtask-vm-dotenv-test-missing-root");
        let error = load_guest_credentials(&missing_root).expect_err("no .env there");
        assert!(error.contains(".env"));
    }
}
