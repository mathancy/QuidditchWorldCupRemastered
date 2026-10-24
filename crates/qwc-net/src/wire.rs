//! The datagrams. Each starts with `QWCN`, the protocol's version (`u16`), the message's kind (`u8`) and the session's
//! id (`u32`: the joiner's nonce, 0 before it has one), then the kind's fields, little-endian; strings and blobs are
//! a `u16` length and their bytes. Anything that does not read to its last byte is dropped.

use std::fmt;

pub const MAGIC: [u8; 4] = *b"QWCN";
/// Raised whenever a message changes shape: peers on different versions refuse each other at the hello.
pub const PROTOCOL: u16 = 2;
/// The most a datagram carries: under the smallest MTU an IPv6 path may have (1280) less its headers, so nothing is
/// ever fragmented.
pub const MAX_DATAGRAM: usize = 1200;
const HEADER: usize = 4 + 2 + 1 + 4;

/// What the peers say to each other, in the order a session uses them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// Joiner to host, until welcomed: its build (the host refuses another), its nonce (the session's id from then
    /// on) and what it asks for (the game's bytes: the team it wants to play).
    Hello {
        build: String,
        nonce: u32,
        ask: Vec<u8>,
    },
    /// Host to joiner: the match both will play (the game's bytes: the teams, the stadium, the seed).
    Welcome {
        setup: Vec<u8>,
    },
    /// Joiner to host: the match is built; its checksum at the start, which the host's must equal (the same data).
    Ready {
        start_sum: u64,
    },
    /// Host to joiner and back, to time the round trip before the start.
    Ping {
        stamp: u32,
    },
    Pong {
        stamp: u32,
    },
    /// Host to joiner: play, each side's input this many frames late.
    Start {
        delay: u8,
    },
    /// Either way, all through the match: the sender's frames not yet acknowledged, and its news.
    Frames(Frames),
    /// Either way: the session is over (left, refused, the data differs), and why.
    Bye {
        reason: String,
    },
    /// Anyone to a host's port, before joining: is a host there? Answered with [`Message::Lobby`] (session id 0).
    Query {
        stamp: u32,
    },
    /// A waiting host's answer to a query: its build, the game's bytes about the match it offers (the team, the
    /// stadium) and the query's stamp back.
    Lobby {
        build: String,
        about: Vec<u8>,
        stamp: u32,
    },
}

/// A run of the sender's input frames and what it knows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frames {
    /// The first frame's number, and the frames, each `len` bytes.
    pub first: u32,
    pub len: u8,
    pub data: Vec<u8>,
    /// How many of the receiver's frames the sender holds (all below this).
    pub ack: u32,
    /// The frame the sender runs next (to pace the two).
    pub running: u32,
    /// The sender's checksum after a frame it ran: `(frame, sum)`.
    pub sum: Option<(u32, u64)>,
    /// The sender's clock (milliseconds), the receiver's last stamp it saw and how long it held it before this
    /// (the round trip, without a ping).
    pub stamp: u32,
    pub echo: Option<(u32, u16)>,
}

impl Frames {
    pub fn count(&self) -> usize {
        if self.len == 0 {
            0
        } else {
            self.data.len() / usize::from(self.len)
        }
    }

    /// Frame `k` of the run.
    pub fn frame(&self, k: usize) -> &[u8] {
        let n = usize::from(self.len);
        &self.data[k * n..(k + 1) * n]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// Not ours (another program's datagram on the port).
    NotOurs,
    /// Ours, from another version of the protocol.
    Protocol(u16),
    /// Ours, but cut short, too long, or of an unknown kind.
    Malformed,
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            WireError::NotOurs => write!(f, "not a QWC datagram"),
            WireError::Protocol(v) => write!(f, "protocol {v} (this build speaks {PROTOCOL})"),
            WireError::Malformed => write!(f, "a malformed datagram"),
        }
    }
}

const HELLO: u8 = 1;
const WELCOME: u8 = 2;
const READY: u8 = 3;
const PING: u8 = 4;
const PONG: u8 = 5;
const START: u8 = 6;
const FRAMES: u8 = 7;
const BYE: u8 = 8;
const QUERY: u8 = 9;
const LOBBY: u8 = 10;

/// The datagram for `msg` in session `id`.
pub fn encode(id: u32, msg: &Message) -> Vec<u8> {
    let mut w = Writer(Vec::with_capacity(64));
    w.0.extend_from_slice(&MAGIC);
    w.u16(PROTOCOL);
    let kind = match msg {
        Message::Hello { .. } => HELLO,
        Message::Welcome { .. } => WELCOME,
        Message::Ready { .. } => READY,
        Message::Ping { .. } => PING,
        Message::Pong { .. } => PONG,
        Message::Start { .. } => START,
        Message::Frames(_) => FRAMES,
        Message::Bye { .. } => BYE,
        Message::Query { .. } => QUERY,
        Message::Lobby { .. } => LOBBY,
    };
    w.u8(kind);
    w.u32(id);
    match msg {
        Message::Hello { build, nonce, ask } => {
            w.bytes(build.as_bytes());
            w.u32(*nonce);
            w.bytes(ask);
        }
        Message::Welcome { setup } => w.bytes(setup),
        Message::Ready { start_sum } => w.u64(*start_sum),
        Message::Ping { stamp } | Message::Pong { stamp } => w.u32(*stamp),
        Message::Start { delay } => w.u8(*delay),
        Message::Frames(f) => {
            w.u32(f.first);
            w.u8(f.len);
            w.u8(f.count() as u8);
            w.0.extend_from_slice(&f.data);
            w.u32(f.ack);
            w.u32(f.running);
            match f.sum {
                Some((frame, sum)) => {
                    w.u8(1);
                    w.u32(frame);
                    w.u64(sum);
                }
                None => w.u8(0),
            }
            w.u32(f.stamp);
            match f.echo {
                Some((stamp, held)) => {
                    w.u8(1);
                    w.u32(stamp);
                    w.u16(held);
                }
                None => w.u8(0),
            }
        }
        Message::Bye { reason } => w.bytes(reason.as_bytes()),
        Message::Query { stamp } => w.u32(*stamp),
        Message::Lobby { build, about, stamp } => {
            w.bytes(build.as_bytes());
            w.bytes(about);
            w.u32(*stamp);
        }
    }
    w.0
}

/// The session id and message in a datagram.
pub fn decode(b: &[u8]) -> Result<(u32, Message), WireError> {
    if b.len() < 4 || b[..4] != MAGIC {
        return Err(WireError::NotOurs);
    }
    if b.len() < HEADER {
        return Err(WireError::Malformed);
    }
    let mut r = Reader { b, at: 4 };
    let version = r.u16()?;
    if version != PROTOCOL {
        return Err(WireError::Protocol(version));
    }
    let kind = r.u8()?;
    let id = r.u32()?;
    let msg = match kind {
        HELLO => Message::Hello { build: r.string()?, nonce: r.u32()?, ask: r.bytes()?.to_vec() },
        WELCOME => Message::Welcome { setup: r.bytes()?.to_vec() },
        READY => Message::Ready { start_sum: r.u64()? },
        PING => Message::Ping { stamp: r.u32()? },
        PONG => Message::Pong { stamp: r.u32()? },
        START => Message::Start { delay: r.u8()? },
        FRAMES => {
            let first = r.u32()?;
            let len = r.u8()?;
            let count = r.u8()?;
            let data = r.take(usize::from(len) * usize::from(count))?.to_vec();
            let ack = r.u32()?;
            let running = r.u32()?;
            let sum = match r.u8()? {
                0 => None,
                1 => Some((r.u32()?, r.u64()?)),
                _ => return Err(WireError::Malformed),
            };
            let stamp = r.u32()?;
            let echo = match r.u8()? {
                0 => None,
                1 => Some((r.u32()?, r.u16()?)),
                _ => return Err(WireError::Malformed),
            };
            Message::Frames(Frames { first, len, data, ack, running, sum, stamp, echo })
        }
        BYE => Message::Bye { reason: r.string()? },
        QUERY => Message::Query { stamp: r.u32()? },
        LOBBY => Message::Lobby { build: r.string()?, about: r.bytes()?.to_vec(), stamp: r.u32()? },
        _ => return Err(WireError::Malformed),
    };
    if r.at != b.len() {
        return Err(WireError::Malformed);
    }
    Ok((id, msg))
}

/// How many frames of `len` bytes fit a datagram with the rest of a [`Frames`] message.
pub fn frames_that_fit(len: usize) -> usize {
    // the header, then first, len, count, ack, running, the sum, the stamp and the echo
    let fixed = HEADER + 4 + 1 + 1 + 4 + 4 + 13 + 4 + 7;
    if len == 0 {
        return 255;
    }
    ((MAX_DATAGRAM - fixed) / len).min(255)
}

struct Writer(Vec<u8>);

impl Writer {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    /// At most `u16::MAX` bytes (longer is cut: strings and setups are far shorter).
    fn bytes(&mut self, v: &[u8]) {
        let v = &v[..v.len().min(usize::from(u16::MAX))];
        self.u16(v.len() as u16);
        self.0.extend_from_slice(v);
    }
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], WireError> {
        let s = self.b.get(self.at..self.at + n).ok_or(WireError::Malformed)?;
        self.at += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, WireError> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, WireError> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, WireError> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> Result<&'a [u8], WireError> {
        let n = self.u16()?;
        self.take(usize::from(n))
    }
    fn string(&mut self) -> Result<String, WireError> {
        String::from_utf8(self.bytes()?.to_vec()).map_err(|_| WireError::Malformed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_kind() -> Vec<Message> {
        vec![
            Message::Hello { build: "0.1.0".into(), nonce: 0xdead_beef, ask: b"France".to_vec() },
            Message::Welcome { setup: vec![1, 2, 3] },
            Message::Ready { start_sum: 0x0123_4567_89ab_cdef },
            Message::Ping { stamp: 7 },
            Message::Pong { stamp: 7 },
            Message::Start { delay: 3 },
            Message::Frames(Frames {
                first: 1000,
                len: 4,
                data: vec![1, 2, 3, 4, 5, 6, 7, 8],
                ack: 998,
                running: 997,
                sum: Some((996, 42)),
                stamp: 123_456,
                echo: Some((654_321, 12)),
            }),
            Message::Frames(Frames {
                first: 0,
                len: 4,
                data: vec![],
                ack: 0,
                running: 0,
                sum: None,
                stamp: 0,
                echo: None,
            }),
            Message::Bye { reason: "left".into() },
            Message::Query { stamp: 5 },
            Message::Lobby { build: "0.1.0/1".into(), about: b"team England".to_vec(), stamp: 5 },
        ]
    }

    #[test]
    fn every_kind_reads_back() {
        for m in every_kind() {
            let b = encode(9, &m);
            assert_eq!(decode(&b), Ok((9, m.clone())), "{m:?}");
            assert!(b.len() <= MAX_DATAGRAM);
        }
    }

    #[test]
    fn short_long_and_foreign_datagrams_are_refused() {
        for m in every_kind() {
            let b = encode(1, &m);
            for cut in 0..b.len() {
                assert!(decode(&b[..cut]).is_err(), "{m:?} cut at {cut}");
            }
            let mut long = b.clone();
            long.push(0);
            assert_eq!(decode(&long), Err(WireError::Malformed));
        }
        assert_eq!(decode(b"GET / HTTP/1.1"), Err(WireError::NotOurs));
        let mut other = encode(1, &Message::Ping { stamp: 1 });
        other[4] = 99;
        assert_eq!(decode(&other), Err(WireError::Protocol(99)));
    }

    #[test]
    fn a_full_run_of_frames_fits_a_datagram() {
        for len in [1usize, 16, 40, 64, 200] {
            let n = frames_that_fit(len);
            let f = Frames {
                first: u32::MAX,
                len: len as u8,
                data: vec![0xff; n * len],
                ack: u32::MAX,
                running: u32::MAX,
                sum: Some((u32::MAX, u64::MAX)),
                stamp: u32::MAX,
                echo: Some((u32::MAX, u16::MAX)),
            };
            let b = encode(u32::MAX, &Message::Frames(f));
            assert!(b.len() <= MAX_DATAGRAM, "{len}: {}", b.len());
            let more = (n + 1) * len + (b.len() - n * len);
            assert!(n == 255 || more > MAX_DATAGRAM, "{len}: room for {}", n + 1);
        }
    }
}
