//! Theme packages: a theme's directory zipped (`phase6-design.md`,
//! "Packaging"). Only the files a theme is made of are read or written:
//! plain file names, no directories, within fixed limits, so a package can
//! neither write outside the theme's directory nor fill the disk.

use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use verbatim_model::is_plain_file_name;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// The most files a package may hold.
const MAX_FILES: usize = 256;

/// The largest file a package may hold, unpacked.
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// The most a package may hold altogether, unpacked.
const MAX_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

/// One file of a package: its plain file name and its contents.
pub(crate) type PackedFile = (String, Vec<u8>);

/// Reads the files of the package at `path` whose names `wanted` accepts.
///
/// A package holds its files at its top level, or inside one top-level
/// folder, as zipping a directory makes it; files `manifest` locates the
/// package by. Anything else (other folders, names with a directory in
/// them) is left out.
pub(crate) fn read(
    path: &Path,
    manifest: &str,
    wanted: impl Fn(&str) -> bool,
) -> Result<Vec<PackedFile>, String> {
    let file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut archive =
        ZipArchive::new(io::BufReader::new(file)).map_err(|error| error.to_string())?;
    if archive.len() > MAX_FILES {
        return Err(format!("more than {MAX_FILES} files"));
    }
    let names: Vec<String> = archive.file_names().map(str::to_owned).collect();
    let prefix = if names.iter().any(|name| name == manifest) {
        String::new()
    } else {
        let mut folders = names
            .iter()
            .filter_map(|name| name.split_once('/').map(|(folder, _)| folder))
            .collect::<Vec<_>>();
        folders.sort_unstable();
        folders.dedup();
        match folders.as_slice() {
            [folder]
                if names
                    .iter()
                    .any(|name| *name == format!("{folder}/{manifest}")) =>
            {
                format!("{folder}/")
            }
            _ => return Err(format!("no {manifest}")),
        }
    };
    let mut files = Vec::new();
    let mut total = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        if entry.is_dir() {
            continue;
        }
        let Some(name) = entry.name().strip_prefix(&prefix).map(str::to_owned) else {
            continue;
        };
        if !is_plain_file_name(&name) || !wanted(&name) {
            continue;
        }
        let mut contents = Vec::new();
        (&mut entry)
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut contents)
            .map_err(|error| format!("{name}: {error}"))?;
        let size = contents.len() as u64;
        if size > MAX_FILE_BYTES {
            return Err(format!("{name} is larger than {MAX_FILE_BYTES} bytes"));
        }
        total += size;
        if total > MAX_TOTAL_BYTES {
            return Err(format!("more than {MAX_TOTAL_BYTES} bytes"));
        }
        files.push((name, contents));
    }
    Ok(files)
}

/// Writes a package at `path` holding `files`, each a plain file name and
/// the file on disk it is read from, at the package's top level.
pub(crate) fn write(path: &Path, files: &[(String, PathBuf)]) -> Result<(), String> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    let result = write_to(&temp, files).and_then(|()| {
        // On Windows, std::fs::rename replaces an existing destination.
        fs::rename(&temp, path).map_err(|error| error.to_string())
    });
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn write_to(path: &Path, files: &[(String, PathBuf)]) -> Result<(), String> {
    let file = fs::File::create(path).map_err(|error| error.to_string())?;
    let mut zip = ZipWriter::new(io::BufWriter::new(file));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, source) in files {
        let contents = fs::read(source).map_err(|error| format!("{name}: {error}"))?;
        zip.start_file(name.as_str(), options)
            .map_err(|error| error.to_string())?;
        zip.write_all(&contents)
            .map_err(|error| format!("{name}: {error}"))?;
    }
    let mut writer = zip.finish().map_err(|error| error.to_string())?;
    writer.flush().map_err(|error| error.to_string())
}
