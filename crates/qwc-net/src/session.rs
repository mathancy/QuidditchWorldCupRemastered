//! The sessions over UDP: the host waiting on its port, the joiner's hello, the handshake that agrees the match, and
//! the running [`Session`] that carries the frames, times the round trip and notices the other side leaving.
//!
//! The handshake: the joiner says hello (its build, its nonce, what it asks for) until the host welcomes it with the
//! match's setup; it builds the match and answers ready with the match's checksum at the start, which must equal the
//! host's (both have the same data); the host pings it a few times to time the round trip, chooses the input delay
//! from it and says start; the joiner starts at once, the host half a round trip later, so both run frame 0 at about
//! the same moment.

use std::io;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use crate::lockstep::Lockstep;
use crate::stun;
use crate::wire::{self, Frames, Message};

/// The game's tick: a frame each.
pub const TICK: Duration = Duration::from_nanos(1_000_000_000 / 24);
/// Nothing heard for this long: the other side is gone.
pub const LOST_AFTER: Duration = Duration::from_secs(10);
/// The hello and the ready are said again this often until answered.
const RESAY: Duration = Duration::from_millis(250);
/// How long the joiner waits for the host's welcome (time for the host to let it in, [`Host::punch`]).
pub const JOIN_WAIT: Duration = Duration::from_secs(60);
/// Each side waits this long for the other to load the match: the host for the joiner's ready, the joiner for the
/// start.
const READY_WAIT: Duration = Duration::from_secs(90);
/// The pings before the start.
const PINGS: usize = 5;
/// The most input delay chosen on its own (a third of a second).
pub const MAX_DELAY: u32 = 8;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// The address did not name a machine.
    Address(String),
    /// No answer in time: `what` was waited for.
    TimedOut(&'static str),
    /// The other side said no (or bye), and why.
    Refused(String),
    /// The joiner's match did not start as the host's: their game files differ.
    DataDiffers,
    /// The joiner's game could not build the match from the setup.
    Setup(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::Address(a) => write!(f, "no machine at {a}"),
            Error::TimedOut(what) => write!(f, "no answer: {what}"),
            Error::Refused(why) => write!(f, "refused: {why}"),
            Error::DataDiffers => write!(f, "the two games' files differ (another edition, or other gameplay packs)"),
            Error::Setup(e) => write!(f, "the match could not be set up: {e}"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::Io(e)
    }
}

/// How the session ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    /// The other side said bye, and why.
    Left(String),
    /// Nothing came from the other side for [`LOST_AFTER`].
    Lost,
    /// The two matches differed after this frame.
    Desync(u32),
    /// We closed it.
    Closed,
}

impl std::fmt::Display for End {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            End::Left(why) => write!(f, "the other player left ({why})"),
            End::Lost => write!(f, "the connection was lost"),
            End::Desync(n) => write!(f, "the two games went out of step at frame {n}"),
            End::Closed => write!(f, "closed"),
        }
    }
}

/// A datagram from the socket, if one is waiting (`None` when none is, or on the errors a UDP socket reports for an
/// earlier send, such as Windows' "connection reset" after a port-unreachable).
fn receive(socket: &UdpSocket, buf: &mut [u8]) -> io::Result<Option<(usize, SocketAddr)>> {
    loop {
        match socket.recv_from(buf) {
            Ok(r) => return Ok(Some(r)),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => return Ok(None),
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
            Err(e) => return Err(e),
        }
    }
}

/// A nonce no other joiner is likely to have: the clock and the process, mixed.
fn nonce() -> u32 {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64;
    let mut x = t ^ (u64::from(std::process::id()) << 32);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    (x as u32).max(1)
}

/// The input delay for a round trip: half of it, a margin for jitter, in whole frames, at least one.
pub fn delay_for(rtt: Duration) -> u32 {
    let one_way = rtt / 2 + Duration::from_millis(12);
    let frames = one_way.as_nanos().div_ceil(TICK.as_nanos()) as u32;
    frames.clamp(1, MAX_DELAY)
}

/// A joiner knocking: who, its nonce and what it asks for.
#[derive(Clone, Debug)]
pub struct Request {
    pub from: SocketAddr,
    pub nonce: u32,
    pub ask: Vec<u8>,
    /// The host's socket it came to.
    via: usize,
}

/// What came to the host's port.
#[derive(Clone, Debug)]
pub enum Knock {
    /// A joiner of this build.
    Join(Request),
    /// A joiner of another build, told so and turned away.
    OtherBuild { from: SocketAddr, build: String },
}

/// The host, waiting for a joiner on its port, on IPv4 and, where the machine has it, IPv6.
pub struct Host {
    /// IPv4's socket and IPv6's (on Linux an IPv6 socket takes IPv4 too, and is the only one).
    sockets: Vec<UdpSocket>,
    build: String,
    /// What a query is told about the match offered (the game's bytes, [`Host::set_about`]).
    about: Vec<u8>,
}

impl Host {
    /// Listens on UDP `port` on every address of the machine, IPv4 and IPv6 (0: a port the system picks, the same
    /// for both).
    pub fn bind(port: u16, build: &str) -> io::Result<Host> {
        // IPv6 first: where its socket takes IPv4 too (Linux), IPv4's own bind then finds the port taken
        let v6 = UdpSocket::bind(("::", port));
        let port = match &v6 {
            Ok(s) => s.local_addr()?.port(),
            Err(_) => port,
        };
        let sockets = match (UdpSocket::bind(("0.0.0.0", port)), v6) {
            (Ok(v4), Ok(v6)) => vec![v4, v6],
            (Ok(v4), Err(_)) => vec![v4],
            (Err(e), Ok(v6)) if e.kind() == io::ErrorKind::AddrInUse => vec![v6],
            (Err(e), _) => return Err(e),
        };
        for s in &sockets {
            s.set_nonblocking(true)?;
        }
        Ok(Host { sockets, build: build.into(), about: Vec::new() })
    }

    pub fn port(&self) -> u16 {
        self.sockets[0].local_addr().map(|a| a.port()).unwrap_or(0)
    }

    /// What queries ([`query`]) are told about the match offered (the game's bytes).
    pub fn set_about(&mut self, about: &[u8]) {
        self.about = about.to_vec();
    }

    /// Whether IPv6 joiners can come (the machine has IPv6).
    pub fn ipv6(&self) -> bool {
        self.sockets.iter().any(|s| s.local_addr().is_ok_and(|a| a.is_ipv6()))
    }

    /// The socket that sends to `to`: IPv4's for an IPv4 address (or the IPv6 one taking both, the address mapped).
    fn socket_for(&self, to: SocketAddr) -> Option<(&UdpSocket, SocketAddr)> {
        let v6 = |s: &&UdpSocket| s.local_addr().is_ok_and(|a| a.is_ipv6());
        match to {
            SocketAddr::V6(_) => self.sockets.iter().find(v6).map(|s| (s, to)),
            SocketAddr::V4(a) => match self.sockets.iter().find(|s| !v6(s)) {
                Some(s) => Some((s, to)),
                None => {
                    self.sockets.first().map(|s| (s, SocketAddr::new(IpAddr::V6(a.ip().to_ipv6_mapped()), a.port())))
                }
            },
        }
    }

    /// The host's address on the internet over IPv4, from public STUN servers (`stun`), within about `wait` a
    /// server: what a joiner on the internet connects to when the router forwards the port.
    pub fn public_address(&self, wait: Duration) -> stun::Mapping {
        let v4 = self.sockets.iter().find(|s| s.local_addr().is_ok_and(|a| a.is_ipv4()));
        let Some(socket) = v4.or(self.sockets.first()) else { return stun::Mapping::default() };
        let _ = socket.set_nonblocking(false);
        let m = stun::public_address(socket, &stun::SERVERS, wait);
        let _ = socket.set_nonblocking(true);
        m
    }

    /// Opens the way in for a joiner at `to` (its public address) where the host's router would drop what it sends
    /// first: a datagram to it makes the router let its answers in (a punched hole). Said again until it comes.
    pub fn punch(&self, to: SocketAddr) {
        if let Some((s, to)) = self.socket_for(to) {
            let _ = s.send_to(&wire::encode(0, &Message::Ping { stamp: 0 }), to);
        }
    }

    /// The next joiner's hello within `wait`; `None` if none came.
    pub fn poll(&mut self, wait: Duration) -> io::Result<Option<Knock>> {
        let until = Instant::now() + wait;
        let mut buf = [0u8; 2048];
        loop {
            let mut heard = false;
            for (via, s) in self.sockets.iter().enumerate() {
                let Some((n, from)) = receive(s, &mut buf)? else { continue };
                heard = true;
                let msg = wire::decode(&buf[..n]);
                // a query is answered at once: a host is here, its build and its match
                if let Ok((_, Message::Query { stamp })) = msg {
                    let lobby = Message::Lobby { build: self.build.clone(), about: self.about.clone(), stamp };
                    let _ = s.send_to(&wire::encode(0, &lobby), from);
                    continue;
                }
                let Ok((_, Message::Hello { build, nonce, ask })) = msg else { continue };
                if build != self.build {
                    let reason = format!("the host plays version {}, you {build}", self.build);
                    let _ = s.send_to(&wire::encode(nonce, &Message::Bye { reason }), from);
                    return Ok(Some(Knock::OtherBuild { from, build }));
                }
                return Ok(Some(Knock::Join(Request { from, nonce, ask, via })));
            }
            if Instant::now() >= until {
                return Ok(None);
            }
            if !heard {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// Turns a joiner away, telling it why.
    pub fn refuse(&self, r: &Request, reason: &str) {
        let bye = wire::encode(r.nonce, &Message::Bye { reason: reason.into() });
        for _ in 0..3 {
            let _ = self.sockets[r.via].send_to(&bye, r.from);
        }
    }

    /// Plays with the joiner of `r`: [`Host::welcome`], then [`Welcomed::start`] at once.
    pub fn accept(
        self,
        r: Request,
        setup: &[u8],
        start_sum: u64,
        frame_len: usize,
        delay: Option<u32>,
    ) -> Result<Session, Error> {
        self.welcome(r, setup)?.start(start_sum, frame_len, delay)
    }

    /// Takes the joiner of `r`: sends it `setup` (the match both will play), which it builds while the host loads
    /// its own; then [`Welcomed::start`].
    pub fn welcome(mut self, r: Request, setup: &[u8]) -> io::Result<Welcomed> {
        let socket = self.sockets.swap_remove(r.via);
        socket.set_nonblocking(false)?;
        let w = Welcomed { socket, peer: r.from, id: r.nonce, setup: setup.to_vec() };
        for _ in 0..3 {
            w.send(&Message::Welcome { setup: w.setup.clone() })?;
        }
        Ok(w)
    }
}

/// The bye's reason when the two matches' starts differ (the joiner reads it as [`Error::DataDiffers`]).
const FILES_DIFFER: &str = "the two games' files differ";

/// The host with a joiner welcomed, both loading the match.
pub struct Welcomed {
    socket: UdpSocket,
    peer: SocketAddr,
    id: u32,
    setup: Vec<u8>,
}

impl Welcomed {
    fn send(&self, m: &Message) -> io::Result<usize> {
        self.socket.send_to(&wire::encode(self.id, m), self.peer)
    }

    /// Who joined.
    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// Waits for the joiner's match to be ready (the welcome said again meanwhile, in case it was lost) with its
    /// checksum at the start, which must be `start_sum`; times the round trip and starts, with `delay` frames of
    /// input delay (`None`: from the round trip, [`delay_for`]). Frames are `frame_len` bytes. The host is team 0's
    /// side (seat 0).
    pub fn start(self, start_sum: u64, frame_len: usize, delay: Option<u32>) -> Result<Session, Error> {
        let (socket, peer, id) = (&self.socket, self.peer, self.id);
        let welcome = Message::Welcome { setup: self.setup.clone() };
        let mut buf = [0u8; 2048];
        let began = Instant::now();
        // the welcome, until the ready
        let mut said = None::<Instant>;
        let their_sum = loop {
            if began.elapsed() > READY_WAIT {
                return Err(Error::TimedOut("the joiner's game to load the match"));
            }
            if said.is_none_or(|t| t.elapsed() >= RESAY) {
                self.send(&welcome)?;
                said = Some(Instant::now());
            }
            socket.set_read_timeout(Some(RESAY / 5))?;
            let Some((n, from)) = receive(socket, &mut buf)? else { continue };
            match wire::decode(&buf[..n]) {
                Ok((i, Message::Ready { start_sum })) if from == peer && i == id => break start_sum,
                Ok((i, Message::Bye { reason })) if from == peer && i == id => return Err(Error::Refused(reason)),
                _ => {}
            }
        };
        if their_sum != start_sum {
            for _ in 0..3 {
                self.send(&Message::Bye { reason: FILES_DIFFER.into() })?;
            }
            return Err(Error::DataDiffers);
        }
        // the round trip: the slowest of a few pings
        let mut rtt = Duration::ZERO;
        let mut answered = 0;
        for k in 0..PINGS as u32 {
            let sent = Instant::now();
            self.send(&Message::Ping { stamp: k })?;
            while sent.elapsed() < Duration::from_secs(1) {
                socket.set_read_timeout(Some(Duration::from_millis(20)))?;
                let Some((n, from)) = receive(socket, &mut buf)? else { continue };
                if let Ok((i, Message::Pong { stamp })) = wire::decode(&buf[..n]) {
                    if from == peer && i == id && stamp == k {
                        rtt = rtt.max(sent.elapsed());
                        answered += 1;
                        break;
                    }
                }
            }
        }
        if answered == 0 {
            return Err(Error::TimedOut("the joiner's answer to a ping"));
        }
        let delay = delay.unwrap_or_else(|| delay_for(rtt)).min(255) as u8;
        for _ in 0..2 {
            self.send(&Message::Start { delay })?;
        }
        // the start reaches the joiner half a round trip from now
        std::thread::sleep(rtt / 2);
        let mut s = Session::new(self.socket, peer, id, 0, Lockstep::new(frame_len, u32::from(delay)));
        s.rtt = Some(rtt);
        s.start = Some(delay);
        Ok(s)
    }
}

/// Joins the host at `to`: [`Joining::bind`] and [`Joining::hello`], builds the match from the host's setup with `build_match` (which
/// answers the match's checksum at the start), then [`Joining::ready`].
pub fn join(
    to: &str,
    default_port: u16,
    build: &str,
    ask: &[u8],
    frame_len: usize,
    build_match: impl FnOnce(&[u8]) -> Result<u64, String>,
) -> Result<Session, Error> {
    let j = Joining::bind(to, default_port)?;
    let setup = j.hello(build, ask, JOIN_WAIT)?;
    let start_sum = build_match(&setup).map_err(Error::Setup)?;
    j.ready(start_sum, frame_len)
}

/// The joiner: its socket, the host, its nonce.
pub struct Joining {
    socket: UdpSocket,
    peer: SocketAddr,
    id: u32,
}

impl Joining {
    fn send(&self, m: &Message) -> io::Result<usize> {
        self.socket.send_to(&wire::encode(self.id, m), self.peer)
    }

    /// The host.
    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// A socket to join the host at `to` (`address:port` or `name:port`; the port defaults to `default_port`).
    pub fn bind(to: &str, default_port: u16) -> Result<Joining, Error> {
        let peer = resolve(to, default_port)?;
        let socket = UdpSocket::bind(if peer.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" })?;
        Ok(Joining { socket, peer, id: nonce() })
    }

    /// This side's address on the internet, from the socket the hello goes out on (`stun`): what the host lets in
    /// ([`Host::punch`]) when its router drops the hello.
    pub fn public_address(&self, wait: Duration) -> stun::Mapping {
        stun::public_address(&self.socket, &stun::SERVERS, wait)
    }

    /// Says hello with `build` and `ask` until the host welcomes us (at most `wait`, [`JOIN_WAIT`] for players):
    /// answers the match's setup.
    pub fn hello(&self, build: &str, ask: &[u8], wait: Duration) -> Result<Vec<u8>, Error> {
        let hello = Message::Hello { build: build.into(), nonce: self.id, ask: ask.to_vec() };
        let mut buf = [0u8; 2048];
        let began = Instant::now();
        let mut said = None::<Instant>;
        loop {
            if began.elapsed() > wait {
                return Err(Error::TimedOut("the host (is it hosting, and can its port be reached?)"));
            }
            if said.is_none_or(|t| t.elapsed() >= RESAY) {
                self.send(&hello)?;
                said = Some(Instant::now());
            }
            self.socket.set_read_timeout(Some(RESAY / 5))?;
            let Some((n, from)) = receive(&self.socket, &mut buf)? else { continue };
            match wire::decode(&buf[..n]) {
                Ok((i, Message::Welcome { setup })) if from == self.peer && i == self.id => return Ok(setup),
                Ok((i, Message::Bye { reason })) if from == self.peer && i == self.id => {
                    return Err(Error::Refused(reason))
                }
                _ => {}
            }
        }
    }

    /// The match is built (its checksum at the start `start_sum`): says ready until the host starts (it may still
    /// be loading), answering its pings. Frames are `frame_len` bytes. The joiner is team 1's side (seat 1).
    pub fn ready(self, start_sum: u64, frame_len: usize) -> Result<Session, Error> {
        let ready = Message::Ready { start_sum };
        let mut buf = [0u8; 2048];
        let began = Instant::now();
        let mut said = None::<Instant>;
        let delay = loop {
            if began.elapsed() > READY_WAIT {
                return Err(Error::TimedOut("the host to start"));
            }
            if said.is_none_or(|t| t.elapsed() >= RESAY) {
                self.send(&ready)?;
                said = Some(Instant::now());
            }
            self.socket.set_read_timeout(Some(Duration::from_millis(5)))?;
            let Some((n, from)) = receive(&self.socket, &mut buf)? else { continue };
            if from != self.peer {
                continue;
            }
            match wire::decode(&buf[..n]) {
                Ok((i, Message::Ping { stamp })) if i == self.id => {
                    self.send(&Message::Pong { stamp })?;
                }
                Ok((i, Message::Start { delay })) if i == self.id => break delay,
                Ok((i, Message::Bye { reason })) if i == self.id => {
                    return Err(if reason == FILES_DIFFER { Error::DataDiffers } else { Error::Refused(reason) })
                }
                _ => {}
            }
        };
        Ok(Session::new(self.socket, self.peer, self.id, 1, Lockstep::new(frame_len, u32::from(delay))))
    }
}

/// What a host answered a query: its build, what it said of its match (the game's bytes) and the round trip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LobbyInfo {
    pub host: SocketAddr,
    pub build: String,
    pub about: Vec<u8>,
    pub rtt: Duration,
}

/// Asks whether a host is waiting at `to` (as [`Joining::bind`] takes it), within `wait`: the check before joining.
/// A host already playing, or one its router does not let the query reach, does not answer.
pub fn query(to: &str, default_port: u16, wait: Duration) -> Result<LobbyInfo, Error> {
    let host = resolve(to, default_port)?;
    let socket = UdpSocket::bind(if host.is_ipv6() { "[::]:0" } else { "0.0.0.0:0" })?;
    let began = Instant::now();
    let mut buf = [0u8; 2048];
    let mut sent = None::<Instant>;
    let mut stamp = 0u32;
    while began.elapsed() < wait {
        // said again every quarter second (UDP may lose it), each with its own stamp
        if sent.is_none_or(|t| t.elapsed() >= RESAY) {
            stamp += 1;
            socket.send_to(&wire::encode(0, &Message::Query { stamp }), host)?;
            sent = Some(Instant::now());
        }
        socket.set_read_timeout(Some(Duration::from_millis(20)))?;
        let Some((n, from)) = receive(&socket, &mut buf)? else { continue };
        if from != host {
            continue;
        }
        match wire::decode(&buf[..n]) {
            Ok((_, Message::Lobby { build, about, stamp: s })) => {
                // the round trip from the latest query, when it is that one's answer
                let rtt = if s == stamp { sent.map_or(Duration::ZERO, |t| t.elapsed()) } else { RESAY };
                return Ok(LobbyInfo { host, build, about, rtt });
            }
            Err(wire::WireError::Protocol(v)) => {
                return Err(Error::Refused(format!("the host speaks another version of the protocol ({v})")))
            }
            _ => {}
        }
    }
    Err(Error::TimedOut("the host (is it hosting and waiting, and can its port be reached?)"))
}

/// `to` as a socket address, `default_port` when it names none.
pub fn resolve(to: &str, default_port: u16) -> Result<SocketAddr, Error> {
    let to = to.trim();
    let bad = || Error::Address(to.into());
    if let Ok(a) = to.parse::<SocketAddr>() {
        return Ok(a);
    }
    if let Ok(ip) = to.trim_start_matches('[').trim_end_matches(']').parse::<std::net::IpAddr>() {
        return Ok(SocketAddr::new(ip, default_port));
    }
    let with_port = if to.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) {
        to.to_string()
    } else {
        format!("{to}:{default_port}")
    };
    let mut all: Vec<SocketAddr> = with_port.to_socket_addrs().map_err(|_| bad())?.collect();
    // IPv4 first: the host listens on it
    all.sort_by_key(|a| a.is_ipv6());
    all.into_iter().next().ok_or_else(bad)
}

/// A match being played with the other side.
pub struct Session {
    socket: UdpSocket,
    peer: SocketAddr,
    id: u32,
    seat: usize,
    lock: Lockstep,
    epoch: Instant,
    /// When the other side was last heard, and when our frames were last sent.
    heard: Instant,
    sent: Option<Instant>,
    /// The other side's latest clock stamp and when it came (echoed for its round trip); the round trip, smoothed.
    echo: Option<(u32, Instant)>,
    rtt: Option<Duration>,
    /// The frame the other side was running at its latest message, and when that came.
    their_running: Option<(u32, Instant)>,
    /// The host's start (its delay), said again if the joiner is still saying ready.
    start: Option<u8>,
    end: Option<End>,
}

impl Session {
    fn new(socket: UdpSocket, peer: SocketAddr, id: u32, seat: usize, lock: Lockstep) -> Session {
        let _ = socket.set_nonblocking(true);
        let now = Instant::now();
        Session {
            socket,
            peer,
            id,
            seat,
            lock,
            epoch: now,
            heard: now,
            sent: None,
            echo: None,
            rtt: None,
            their_running: None,
            start: None,
            end: None,
        }
    }

    /// 0 for the host (team 0), 1 for the joiner (team 1).
    pub fn seat(&self) -> usize {
        self.seat
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// The frames of input delay.
    pub fn delay(&self) -> u32 {
        self.lock.delay()
    }

    /// The frame to run next.
    pub fn frame(&self) -> u32 {
        self.lock.next()
    }

    /// The round trip, as last timed.
    pub fn rtt(&self) -> Option<Duration> {
        self.rtt
    }

    /// How the session ended, once it has.
    pub fn end(&self) -> Option<&End> {
        self.end.as_ref()
    }

    /// How long since the other side was last heard.
    pub fn silence(&self) -> Duration {
        self.heard.elapsed()
    }

    fn send(&self, m: &Message) {
        let _ = self.socket.send_to(&wire::encode(self.id, m), self.peer);
    }

    fn ms(&self) -> u32 {
        self.epoch.elapsed().as_millis() as u32
    }

    /// Reads what came and sends again what the other side still lacks when it is due. Call it often (each frame
    /// drawn).
    pub fn poll(&mut self) {
        if self.end.is_some() {
            return;
        }
        let mut buf = [0u8; 2048];
        while let Ok(Some((n, from))) = receive(&self.socket, &mut buf) {
            if from != self.peer {
                continue;
            }
            let Ok((id, m)) = wire::decode(&buf[..n]) else { continue };
            if id != self.id {
                continue;
            }
            self.heard = Instant::now();
            match m {
                Message::Frames(f) => self.take(&f),
                Message::Bye { reason } => self.end = Some(End::Left(reason)),
                Message::Ready { .. } => {
                    if let Some(delay) = self.start {
                        self.send(&Message::Start { delay });
                    }
                }
                Message::Ping { stamp } => self.send(&Message::Pong { stamp }),
                _ => {}
            }
            if self.end.is_some() {
                return;
            }
        }
        if let Some(n) = self.lock.desync() {
            self.send(&Message::Bye { reason: format!("out of step at frame {n}") });
            self.end = Some(End::Desync(n));
            return;
        }
        if self.heard.elapsed() > LOST_AFTER {
            self.end = Some(End::Lost);
            return;
        }
        if self.sent.is_none_or(|t| t.elapsed() >= TICK / 2) {
            self.send_frames();
        }
    }

    fn take(&mut self, f: &Frames) {
        let now = Instant::now();
        self.lock.receive(f);
        self.their_running = Some((f.running, now));
        self.echo = Some((f.stamp, now));
        if let Some((stamp, held)) = f.echo {
            let sample = self.ms().wrapping_sub(stamp).saturating_sub(u32::from(held));
            if sample < 10_000 {
                let sample = Duration::from_millis(u64::from(sample));
                self.rtt = Some(match self.rtt {
                    Some(r) => (r * 7 + sample) / 8,
                    None => sample,
                });
            }
        }
    }

    fn send_frames(&mut self) {
        let (first, data) = self.lock.outgoing(wire::frames_that_fit(self.lock.frame_len()));
        let echo = self.echo.map(|(stamp, at)| (stamp, at.elapsed().as_millis().min(u128::from(u16::MAX)) as u16));
        let f = Frames {
            first,
            len: self.lock.frame_len() as u8,
            data,
            ack: self.lock.ack(),
            running: self.lock.next(),
            sum: self.lock.latest_sum(),
            stamp: self.ms(),
            echo,
        };
        self.send(&Message::Frames(f));
        self.sent = Some(Instant::now());
    }

    /// Whether our input for the frame `delay` on is still to be given: give it with [`Session::input`] once each
    /// frame run.
    pub fn wants_input(&self) -> bool {
        self.end.is_none() && self.lock.wants_input()
    }

    /// Our input for the next frame without one (the game's bytes, the session's frame length); sent at once.
    pub fn input(&mut self, frame: Vec<u8>) {
        self.lock.push_input(frame);
        self.send_frames();
    }

    /// Both sides' input for the frame to run next, in team order (the host's first), once both are here.
    pub fn ready(&self) -> Option<[Vec<u8>; 2]> {
        if self.end.is_some() {
            return None;
        }
        let (ours, theirs) = self.lock.ready()?;
        Some(if self.seat == 0 { [ours.to_vec(), theirs.to_vec()] } else { [theirs.to_vec(), ours.to_vec()] })
    }

    /// The frame from [`Session::ready`] was run and left the match with checksum `sum`.
    pub fn ran(&mut self, sum: u64) {
        self.lock.advance(sum);
    }

    /// How many frames we run ahead of the other side (negative: behind), from its latest message and the time
    /// since. A side ahead waits on the other's input; the game slows its ticks a little to stay level
    /// ([`Session::tick_length`]).
    pub fn ahead(&self) -> f64 {
        let Some((theirs, at)) = self.their_running else { return 0.0 };
        let since = at.elapsed() + self.rtt.unwrap_or_default() / 2;
        let theirs_now = f64::from(theirs) + since.as_secs_f64() / TICK.as_secs_f64();
        f64::from(self.lock.next()) - theirs_now
    }

    /// The length of our next tick: [`TICK`], stretched by up to a fifth while we run more than a frame ahead.
    pub fn tick_length(&self) -> Duration {
        let ahead = self.ahead();
        if ahead > 1.0 {
            TICK.mul_f64(1.0 + ((ahead - 1.0) * 0.1).min(0.2))
        } else {
            TICK
        }
    }

    /// Ends the session, telling the other side why.
    pub fn close(&mut self, reason: &str) {
        if self.end.is_none() {
            let bye = Message::Bye { reason: reason.into() };
            for _ in 0..3 {
                self.send(&bye);
            }
            self.end = Some(End::Closed);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close("left the game");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_delay_covers_half_the_round_trip() {
        assert_eq!(delay_for(Duration::from_millis(1)), 1, "a LAN");
        assert_eq!(delay_for(Duration::from_millis(60)), 2);
        assert_eq!(delay_for(Duration::from_millis(150)), 3);
        assert_eq!(delay_for(Duration::from_secs(2)), MAX_DELAY);
    }

    #[test]
    fn addresses_with_and_without_ports() {
        let p = |s| resolve(s, 20030).unwrap();
        assert_eq!(p("192.168.1.5"), "192.168.1.5:20030".parse().unwrap());
        assert_eq!(p("192.168.1.5:7000"), "192.168.1.5:7000".parse().unwrap());
        assert_eq!(p("::1"), "[::1]:20030".parse().unwrap());
        assert_eq!(p("[::1]:7000"), "[::1]:7000".parse().unwrap());
        assert_eq!(p("localhost").port(), 20030);
        assert!(resolve("no such host.invalid", 1).is_err());
    }
}
