//! A client connection to Verbatim's control plane (architecture section
//! 10): the `Hello` handshake, correlation-id reply matching, and a
//! transport that is either the well-known named pipe or a TCP socket.
//!
//! Single-threaded by design — one call to [`Client::request`] or
//! [`Client::next_frame`] at a time — which is why sharing the underlying
//! handle between the reader and a fresh writer via `try_clone` is safe
//! here, unlike the server's overlapped-I/O pipe (see `server`'s module
//! doc).

use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};

use crate::protocol::{
    Frame, PIPE_NAME, PROTOCOL_VERSION, ReplyPayload, Request, RequestEnvelope, read_message,
    write_message,
};

/// The two transports a [`Client`] can speak: the local named pipe, or a
/// TCP socket for a Verbatim reachable over the network (or from inside a
/// VM host).
enum Transport {
    Pipe(File),
    Tcp(TcpStream),
}

impl Transport {
    fn try_clone(&self) -> io::Result<Self> {
        match self {
            Self::Pipe(file) => file.try_clone().map(Self::Pipe),
            Self::Tcp(stream) => stream.try_clone().map(Self::Tcp),
        }
    }
}

impl Read for Transport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Pipe(file) => file.read(buf),
            Self::Tcp(stream) => stream.read(buf),
        }
    }
}

impl Write for Transport {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Pipe(file) => file.write(buf),
            Self::Tcp(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Pipe(file) => file.flush(),
            Self::Tcp(stream) => stream.flush(),
        }
    }
}

/// An open connection to Verbatim's control plane, past the `Hello`
/// handshake.
pub struct Client {
    writer: Transport,
    reader: BufReader<Transport>,
    next_id: u64,
}

impl Client {
    /// Opens the well-known control pipe and completes the `Hello`
    /// handshake.
    ///
    /// # Errors
    ///
    /// Returns an error with a clear message if Verbatim is not running
    /// (the pipe does not exist), or if the handshake fails or is refused.
    pub fn connect_pipe() -> io::Result<Self> {
        Self::connect_pipe_named(PIPE_NAME)
    }

    /// Opens `pipe_name` and completes the `Hello` handshake; the override
    /// point tests use to avoid contending with a real, already-running
    /// Verbatim instance.
    ///
    /// # Errors
    ///
    /// Returns an error with a clear message if the pipe does not exist, or
    /// if the handshake fails or is refused.
    pub fn connect_pipe_named(pipe_name: &str) -> io::Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(pipe_name)
            .map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "Verbatim is not running (could not open the control pipe at {pipe_name}: {error})"
                    ),
                )
            })?;
        Self::handshake(Transport::Pipe(file))
    }

    /// Connects to `addr` over TCP and completes the `Hello` handshake —
    /// the transport a remote or in-VM control-plane client uses instead of
    /// the local named pipe.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection cannot be established, or the
    /// handshake fails or is refused.
    pub fn connect_tcp<A: ToSocketAddrs>(addr: A) -> io::Result<Self> {
        let stream = TcpStream::connect(addr)?;
        Self::handshake(Transport::Tcp(stream))
    }

    /// Wraps an already-connected `TcpStream` and completes the `Hello`
    /// handshake, instead of dialing a fresh connection. The seam a
    /// tunneled connection needs: the same socket first speaks a
    /// different protocol (the M2 agent's, to reach `OpenControlTunnel`),
    /// and only after that handoff does it start speaking the control
    /// protocol from this constructor onward, with no reconnect.
    ///
    /// # Errors
    ///
    /// Returns an error if the handshake fails or is refused.
    pub fn from_tcp_stream(stream: TcpStream) -> io::Result<Self> {
        Self::handshake(Transport::Tcp(stream))
    }

    fn handshake(transport: Transport) -> io::Result<Self> {
        let reader = BufReader::new(transport.try_clone()?);
        let mut client = Self {
            writer: transport,
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
///
/// # Errors
///
/// Returns an error if the frame is a [`Frame::Error`].
pub fn ok_or_error(frame: Frame) -> io::Result<Frame> {
    match frame {
        Frame::Error { message, .. } => Err(io::Error::other(message)),
        reply => Ok(reply),
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    /// A minimal fake server (`Hello` then always `Ok`) so this test
    /// exercises `Client`'s TCP transport and framing end to end without a
    /// real Verbatim instance.
    #[test]
    fn client_connects_and_round_trips_over_tcp() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("binds an ephemeral port");
        let addr = listener.local_addr().expect("has a local address");

        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accepts one connection");
            let mut reader = BufReader::new(stream.try_clone().expect("clones the stream"));
            let mut writer = stream;
            let hello: RequestEnvelope = read_message(&mut reader)
                .expect("reads")
                .expect("not end of stream");
            let Request::Hello { .. } = hello.request else {
                panic!("expected Hello first");
            };
            write_message(
                &mut writer,
                &Frame::Reply {
                    to: hello.id,
                    payload: ReplyPayload::Hello {
                        protocol_version: PROTOCOL_VERSION,
                    },
                },
            )
            .expect("writes Hello reply");

            let second: RequestEnvelope = read_message(&mut reader)
                .expect("reads")
                .expect("not end of stream");
            write_message(
                &mut writer,
                &Frame::Reply {
                    to: second.id,
                    payload: ReplyPayload::Ok,
                },
            )
            .expect("writes second reply");
        });

        let mut client = Client::connect_tcp(addr).expect("connects and completes Hello");
        let reply = ok_or_error(client.request(Request::Quit).expect("sends a request"))
            .expect("not an error");
        assert_eq!(
            reply,
            Frame::Reply {
                to: 2,
                payload: ReplyPayload::Ok,
            }
        );

        server.join().expect("server thread does not panic");
    }
}
