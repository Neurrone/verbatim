//! Connection to a running Verbatim instance over the control plane.

use std::fs::{File, OpenOptions};
use std::io::{self, BufReader};

use verbatim_control::protocol::{
    Frame, PIPE_NAME, PROTOCOL_VERSION, ReplyPayload, Request, RequestEnvelope, read_message,
    write_message,
};

/// An open connection to Verbatim's control plane, past the `Hello`
/// handshake.
pub struct Client {
    writer: File,
    reader: BufReader<File>,
    next_id: u64,
}

impl Client {
    /// Opens the control pipe and completes the `Hello` handshake.
    ///
    /// # Errors
    ///
    /// Returns an error with a clear message if Verbatim is not running
    /// (the pipe does not exist), or if the handshake fails or is refused.
    pub fn connect() -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(PIPE_NAME)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "Verbatim is not running (could not open the control pipe at {PIPE_NAME}: {error})"
                    ),
                )
            })?;
        let reader = BufReader::new(file.try_clone()?);
        let mut client = Self {
            writer: file,
            reader,
            next_id: 1,
        };

        match client.request(Request::Hello {
            protocol_version: PROTOCOL_VERSION,
        })? {
            Frame::Reply {
                payload: ReplyPayload::Hello { .. },
                ..
            } => Ok(client),
            Frame::Error { message, .. } => {
                Err(io::Error::other(format!("Hello was refused: {message}")))
            }
            other => Err(io::Error::other(format!(
                "unexpected reply to Hello: {other:?}"
            ))),
        }
    }

    /// Sends one request and waits for its matching reply or error,
    /// discarding any subscription frames (event or speech) that arrive
    /// first on a connection that has not subscribed to them.
    ///
    /// # Errors
    ///
    /// Returns an error if writing fails, the connection closes before a
    /// matching reply arrives, or a message fails to parse.
    pub fn request(&mut self, request: Request) -> io::Result<Frame> {
        let id = self.next_id;
        self.next_id += 1;
        write_message(&mut self.writer, &RequestEnvelope { id, request })?;
        loop {
            match self.next_frame()? {
                frame @ (Frame::Reply { to, .. } | Frame::Error { to, .. }) if to == id => {
                    return Ok(frame);
                }
                _ => {}
            }
        }
    }

    /// Reads the next frame of any kind, for subscription loops
    /// (`watch-events`, `watch-speech`).
    ///
    /// # Errors
    ///
    /// Returns an error if the connection closes or a message fails to
    /// parse.
    pub fn next_frame(&mut self) -> io::Result<Frame> {
        read_message(&mut self.reader)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Verbatim closed the connection",
            )
        })
    }
}

/// Turns a [`Frame::Error`] into a plain `Err`, passing replies through
/// unchanged.
pub fn ok_or_error(frame: Frame) -> io::Result<Frame> {
    match frame {
        Frame::Error { message, .. } => Err(io::Error::other(message)),
        reply => Ok(reply),
    }
}
