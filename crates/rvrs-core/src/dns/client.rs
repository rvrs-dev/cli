//! One question to one server: UDP first, TCP when the answer was truncated. Every call opens
//! its own socket and blocks until the answer or the wait runs out, so callers that ask several
//! servers give each question a thread.

use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

use super::message::{self, MessageError, Presentation, QueryFlags, Response};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    Timeout(String),
    MismatchedId,
    /// The server could not be reached at all, with the system's reason.
    Unreachable(String, String),
    Message(MessageError),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Timeout(server) => write!(formatter, "{server} didn't answer in time."),
            ClientError::MismatchedId => {
                formatter.write_str("The DNS answer didn't match the question.")
            }
            ClientError::Unreachable(server, reason) => {
                write!(formatter, "Couldn't reach {server} ({reason}).")
            }
            ClientError::Message(error) => error.fmt(formatter),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Question<'a> {
    pub name: &'a str,
    pub type_code: u16,
    pub server: SocketAddr,
    pub flags: QueryFlags,
    pub timeout: Duration,
    /// Ask over TCP from the start.
    pub tcp: bool,
    pub presentation: Presentation,
}

impl<'a> Question<'a> {
    /// A question the way the lookup asks it: EDNS with the DO bit, the app's presentation.
    pub fn new(
        name: &'a str,
        type_code: u16,
        server: IpAddr,
        recursive: bool,
        timeout: Duration,
    ) -> Self {
        Self {
            name,
            type_code,
            server: SocketAddr::new(server, 53),
            flags: QueryFlags {
                recursive,
                edns: true,
                dnssec_ok: true,
                authentic_data: false,
            },
            timeout,
            tcp: false,
            presentation: Presentation::App,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Reply {
    pub response: Response,
    pub rtt: Duration,
    pub over_tcp: bool,
    /// Bytes of the answer, as dig reports them.
    pub size: usize,
}

pub fn query(question: &Question) -> Result<Reply, ClientError> {
    let id = random_id();
    let message = message::query(id, question.name, question.type_code, question.flags);
    let started = Instant::now();
    let mut over_tcp = question.tcp;
    let mut data = exchange(&message, question, over_tcp)?;
    let mut response =
        message::parse(&data, question.presentation).map_err(ClientError::Message)?;
    if response.is_truncated() && !over_tcp {
        data = exchange(&message, question, true)?;
        response = message::parse(&data, question.presentation).map_err(ClientError::Message)?;
        over_tcp = true;
    }
    if response.id != id {
        return Err(ClientError::MismatchedId);
    }
    Ok(Reply {
        response,
        rtt: started.elapsed(),
        over_tcp,
        size: data.len(),
    })
}

fn exchange(message: &[u8], question: &Question, tcp: bool) -> Result<Vec<u8>, ClientError> {
    let server = question.server;
    let shown = server.ip().to_string();
    let unreachable = |error: std::io::Error| match error.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
            ClientError::Timeout(shown.clone())
        }
        _ => ClientError::Unreachable(shown.clone(), reason(&error)),
    };
    if tcp {
        let mut stream =
            TcpStream::connect_timeout(&server, question.timeout).map_err(unreachable)?;
        stream
            .set_read_timeout(Some(question.timeout))
            .map_err(unreachable)?;
        stream
            .set_write_timeout(Some(question.timeout))
            .map_err(unreachable)?;
        let mut framed = (message.len() as u16).to_be_bytes().to_vec();
        framed.extend_from_slice(message);
        stream.write_all(&framed).map_err(unreachable)?;
        let mut prefix = [0u8; 2];
        stream.read_exact(&mut prefix).map_err(unreachable)?;
        let mut answer = vec![0u8; usize::from(u16::from_be_bytes(prefix))];
        stream.read_exact(&mut answer).map_err(unreachable)?;
        return Ok(answer);
    }
    let bind: SocketAddr = if server.is_ipv4() {
        ([0, 0, 0, 0], 0).into()
    } else {
        ([0u16; 8], 0).into()
    };
    let socket = UdpSocket::bind(bind).map_err(unreachable)?;
    socket.connect(server).map_err(unreachable)?;
    socket.send(message).map_err(unreachable)?;
    // An answer to an earlier question could arrive on a reused port. Wait for ours.
    let deadline = Instant::now() + question.timeout;
    let mut buffer = vec![0u8; 4096];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(ClientError::Timeout(shown));
        }
        socket.set_read_timeout(Some(left)).map_err(unreachable)?;
        let count = socket.recv(&mut buffer).map_err(unreachable)?;
        if count >= 2 && buffer[..2] == message[..2] {
            buffer.truncate(count);
            return Ok(buffer);
        }
    }
}

/// The system's reason without its error number: "Connection refused".
fn reason(error: &std::io::Error) -> String {
    let text = error.to_string();
    text.split(" (os error").next().unwrap_or(&text).to_string()
}

fn random_id() -> u16 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    (RandomState::new().build_hasher().finish() as u16).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dns::message::tests::Builder;
    use crate::dns::message::{types, Section};

    /// A UDP server on this device that answers one question with what `answer` builds.
    fn serve(answer: impl Fn(u16) -> Vec<u8> + Send + 'static) -> SocketAddr {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let address = socket.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 512];
            if let Ok((_, from)) = socket.recv_from(&mut buffer) {
                let id = u16::from_be_bytes([buffer[0], buffer[1]]);
                let _ = socket.send_to(&answer(id), from);
            }
        });
        address
    }

    fn question(server: SocketAddr) -> Question<'static> {
        let mut question = Question::new(
            "example.com",
            types::A,
            server.ip(),
            true,
            Duration::from_millis(500),
        );
        question.server = server;
        question
    }

    #[test]
    fn an_answer_comes_back_with_its_round_trip() {
        let server = serve(|id| {
            Builder::new(id, 0x8180, "example.com", types::A)
                .record(
                    Section::Answer,
                    "example.com",
                    types::A,
                    300,
                    &[93, 184, 216, 34],
                )
                .build()
        });
        let reply = query(&question(server)).unwrap();
        assert_eq!(reply.response.records[0].value, "93.184.216.34");
        assert!(!reply.over_tcp);
        assert!(reply.size > 12 && reply.rtt < Duration::from_millis(500));
    }

    #[test]
    fn an_answer_to_another_question_is_not_ours() {
        // The server answers with a different id, so the client keeps waiting and times out.
        let server =
            serve(|id| Builder::new(id.wrapping_add(1), 0x8180, "example.com", types::A).build());
        let mut question = question(server);
        question.timeout = Duration::from_millis(150);
        assert_eq!(
            query(&question).unwrap_err(),
            ClientError::Timeout("127.0.0.1".to_string())
        );
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            ClientError::Timeout("9.9.9.9".into()).to_string(),
            "9.9.9.9 didn't answer in time."
        );
        assert_eq!(
            ClientError::Unreachable("10.0.0.1".into(), "Connection refused".into()).to_string(),
            "Couldn't reach 10.0.0.1 (Connection refused)."
        );
        let error = std::io::Error::from_raw_os_error(libc::ECONNREFUSED);
        assert_eq!(reason(&error), "Connection refused");
    }
}
