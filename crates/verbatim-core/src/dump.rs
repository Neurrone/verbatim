//! Flight-recorder dump format (architecture section 9, milestone M2):
//! versioned JSON-lines files a live session (crash or a user-triggered
//! control-plane request) writes to disk, and [`crate::replay`] later
//! replays as a regression test.
//!
//! A dump is a header line followed by one compact-JSON [`RecordedInput`]
//! per line, in the order the flight recorder captured them. The header
//! carries a format version this module checks on read, the crate version
//! that wrote it (informational only), and a timestamp string supplied by
//! the caller rather than read from a clock here, so this crate stays free
//! of I/O and time dependencies. A dump interrupted mid-write (a crash) is
//! expected: [`read_dump`] parses every complete line it can and reports the
//! rest as truncated rather than failing outright.

use std::io::{self, BufRead, Write};

use serde::{Deserialize, Serialize};

use crate::recorder::RecordedInput;

/// Format version this module reads and writes. Bumped whenever the header
/// or per-line shape changes in a way that is not backward compatible.
pub const DUMP_FORMAT_VERSION: u32 = 0;

/// The first line of a dump: format version, the crate version that wrote
/// it, and a caller-supplied timestamp string.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DumpHeader {
    /// The dump format version; see [`DUMP_FORMAT_VERSION`].
    pub format_version: u32,
    /// `CARGO_PKG_VERSION` of the crate that wrote the dump, informational
    /// only.
    pub crate_version: String,
    /// When the dump was written, in whatever format the caller chose; this
    /// crate never reads a clock, so it neither validates nor parses this
    /// field.
    pub timestamp: String,
}

/// The result of reading a dump: its header, every input line that parsed
/// completely, and whether the file ended before a final line finished
/// writing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DumpContents {
    /// The dump's header.
    pub header: DumpHeader,
    /// Every fully parsed input, in recorded order.
    pub inputs: Vec<RecordedInput>,
    /// True when the file ended mid-line: a crash-time dump cut off before
    /// its last record finished writing. The inputs already parsed are
    /// still returned; only the incomplete tail is dropped.
    pub truncated: bool,
}

/// An error reading a dump.
#[derive(Debug)]
pub enum DumpReadError {
    /// An I/O error from the underlying reader.
    Io(io::Error),
    /// The file was empty: no header line at all.
    MissingHeader,
    /// The header line was not valid JSON or not a [`DumpHeader`].
    MalformedHeader(serde_json::Error),
    /// The header parsed but named a format version this build does not
    /// understand.
    UnsupportedVersion {
        /// The version the file declares.
        found: u32,
        /// The version this build reads.
        supported: u32,
    },
    /// A complete (newline-terminated) line was not a valid
    /// [`RecordedInput`]. Only complete lines produce this error; an
    /// incomplete trailing line is reported as [`DumpContents::truncated`]
    /// instead.
    MalformedLine {
        /// 1-based line number within the file, the header counting as
        /// line 1.
        line_number: usize,
        /// The underlying parse error.
        source: serde_json::Error,
    },
}

impl std::fmt::Display for DumpReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "flight-recorder dump I/O error: {error}"),
            Self::MissingHeader => write!(f, "flight-recorder dump is empty: no header line"),
            Self::MalformedHeader(error) => {
                write!(f, "flight-recorder dump header is malformed: {error}")
            }
            Self::UnsupportedVersion { found, supported } => write!(
                f,
                "flight-recorder dump format version {found} is not supported (this build reads version {supported})"
            ),
            Self::MalformedLine {
                line_number,
                source,
            } => write!(
                f,
                "flight-recorder dump line {line_number} is malformed: {source}"
            ),
        }
    }
}

impl std::error::Error for DumpReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::MalformedHeader(error) | Self::MalformedLine { source: error, .. } => Some(error),
            Self::MissingHeader | Self::UnsupportedVersion { .. } => None,
        }
    }
}

impl From<io::Error> for DumpReadError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Writes a dump: a header line, then one compact-JSON [`RecordedInput`] per
/// line, each newline-terminated. `crate_version` and `timestamp` are
/// recorded verbatim in the header.
///
/// # Errors
///
/// Returns any I/O error from `writer`.
pub fn write_dump<W: Write>(
    writer: &mut W,
    crate_version: &str,
    timestamp: &str,
    inputs: &[RecordedInput],
) -> io::Result<()> {
    write_line(
        writer,
        &DumpHeader {
            format_version: DUMP_FORMAT_VERSION,
            crate_version: crate_version.to_owned(),
            timestamp: timestamp.to_owned(),
        },
    )?;
    for input in inputs {
        write_line(writer, input)?;
    }
    Ok(())
}

fn write_line<W: Write, T: Serialize>(writer: &mut W, value: &T) -> io::Result<()> {
    let line = serde_json::to_string(value).map_err(io::Error::other)?;
    writer.write_all(line.as_bytes())?;
    writer.write_all(b"\n")
}

/// Reads a dump: the header, every fully-written [`RecordedInput`] line, and
/// whether the file's last line was cut off mid-write.
///
/// A version mismatch or a malformed *complete* line is a hard error; an
/// incomplete final line (no trailing newline, and the content on it does
/// not parse) is tolerated, since [`write_dump`] always terminates a
/// complete line with a newline and a crash can only ever truncate the last
/// one. Everything parsed before the truncation point is still returned.
///
/// # Errors
///
/// Returns [`DumpReadError`] for I/O failures, a missing or malformed
/// header, an unsupported format version, or a malformed complete line.
pub fn read_dump<R: BufRead>(reader: &mut R) -> Result<DumpContents, DumpReadError> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Err(DumpReadError::MissingHeader);
    }
    let header: DumpHeader =
        serde_json::from_str(line.trim_end()).map_err(DumpReadError::MalformedHeader)?;
    if header.format_version != DUMP_FORMAT_VERSION {
        return Err(DumpReadError::UnsupportedVersion {
            found: header.format_version,
            supported: DUMP_FORMAT_VERSION,
        });
    }

    let mut inputs = Vec::new();
    let mut truncated = false;
    let mut line_number = 1usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        line_number += 1;
        let complete = line.ends_with('\n');
        let trimmed = line.trim_end_matches(['\n', '\r']);

        match serde_json::from_str::<RecordedInput>(trimmed) {
            Ok(input) => {
                inputs.push(input);
                if !complete {
                    // Parsed fine but the file ended without a trailing
                    // newline: the writer never got to terminate this line,
                    // so nothing can have followed it either.
                    truncated = true;
                    break;
                }
            }
            Err(error) => {
                if complete {
                    return Err(DumpReadError::MalformedLine {
                        line_number,
                        source: error,
                    });
                }
                // An incomplete trailing line that also fails to parse: a
                // crash cut it off mid-write. Tolerate it and stop.
                truncated = true;
                break;
            }
        }
    }

    Ok(DumpContents {
        header,
        inputs,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use verbatim_model::{
        Backend, Input, NodeId, NodeSnapshot, NormalizedEvent, Pid, Role, SnapshotVersion,
        StateSet, TraceId,
    };

    fn sample_inputs() -> Vec<RecordedInput> {
        vec![
            RecordedInput {
                input: Input::Event {
                    trace_id: TraceId::mint(),
                    source: Pid(1),
                    backend: Backend::Uia,
                    version: SnapshotVersion(1),
                    event: NormalizedEvent::FocusChanged {
                        node: NodeSnapshot {
                            id: NodeId::new(1),
                            backend: Backend::Uia,
                            role: Role::Button,
                            name: Some("OK".to_owned()),
                            value: None,
                            states: StateSet::new(),
                        },
                    },
                },
                effect_count: 1,
            },
            RecordedInput {
                input: Input::Tick,
                effect_count: 0,
            },
        ]
    }

    #[test]
    fn round_trips_header_and_inputs() {
        let inputs = sample_inputs();
        let mut buffer = Vec::new();
        write_dump(&mut buffer, "9.9.9", "2026-07-14T00:00:00Z", &inputs).expect("writes");

        let mut reader = buffer.as_slice();
        let contents = read_dump(&mut reader).expect("reads");

        assert_eq!(contents.header.format_version, DUMP_FORMAT_VERSION);
        assert_eq!(contents.header.crate_version, "9.9.9");
        assert_eq!(contents.header.timestamp, "2026-07-14T00:00:00Z");
        assert_eq!(contents.inputs, inputs);
        assert!(!contents.truncated);
    }

    #[test]
    fn rejects_a_newer_format_version() {
        let header = DumpHeader {
            format_version: DUMP_FORMAT_VERSION + 1,
            crate_version: "9.9.9".to_owned(),
            timestamp: "2026-07-14T00:00:00Z".to_owned(),
        };
        let mut buffer = Vec::new();
        write_line(&mut buffer, &header).expect("writes header");

        let mut reader = buffer.as_slice();
        match read_dump(&mut reader) {
            Err(DumpReadError::UnsupportedVersion { found, supported }) => {
                assert_eq!(found, DUMP_FORMAT_VERSION + 1);
                assert_eq!(supported, DUMP_FORMAT_VERSION);
            }
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
    }

    #[test]
    fn rejects_a_missing_header() {
        let mut reader: &[u8] = b"";
        match read_dump(&mut reader) {
            Err(DumpReadError::MissingHeader) => {}
            other => panic!("expected MissingHeader, got {other:?}"),
        }
    }

    #[test]
    fn tolerates_a_truncated_trailing_line() {
        let inputs = sample_inputs();
        let mut buffer = Vec::new();
        write_dump(&mut buffer, "9.9.9", "2026-07-14T00:00:00Z", &inputs).expect("writes");

        let third = RecordedInput {
            input: Input::Event {
                trace_id: TraceId::mint(),
                source: Pid(2),
                backend: Backend::Uia,
                version: SnapshotVersion(9),
                event: NormalizedEvent::ValueChanged {
                    node_id: NodeId::new(9),
                    value: Some("cut off here".to_owned()),
                },
            },
            effect_count: 1,
        };
        let full_line = serde_json::to_string(&third).expect("serializes");
        // Simulate a crash mid-write: only the first half of the line made
        // it to disk, with no trailing newline.
        buffer.extend_from_slice(&full_line.as_bytes()[..full_line.len() / 2]);

        let mut reader = buffer.as_slice();
        let contents = read_dump(&mut reader).expect("tolerates the truncated tail");

        assert_eq!(contents.inputs, inputs, "the two complete records survive");
        assert!(contents.truncated);
    }

    #[test]
    fn a_malformed_complete_line_is_a_hard_error() {
        let mut buffer = Vec::new();
        write_line(
            &mut buffer,
            &DumpHeader {
                format_version: DUMP_FORMAT_VERSION,
                crate_version: "9.9.9".to_owned(),
                timestamp: "2026-07-14T00:00:00Z".to_owned(),
            },
        )
        .expect("writes header");
        buffer.extend_from_slice(b"not json at all\n");

        let mut reader = buffer.as_slice();
        match read_dump(&mut reader) {
            Err(DumpReadError::MalformedLine { line_number, .. }) => assert_eq!(line_number, 2),
            other => panic!("expected MalformedLine, got {other:?}"),
        }
    }
}
