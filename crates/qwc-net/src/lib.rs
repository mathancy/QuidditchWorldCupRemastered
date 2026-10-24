//! Online play with no server of ours: one player hosts (listens on a UDP port), the other joins straight to the
//! host's address, and the two machines run the same match side by side in lockstep. Only the players' inputs cross
//! the network, one small frame each a tick; the match itself is computed on both machines from the same data and
//! the same inputs, which works because `qwc-sim` computes the same bits on every platform (`qwc_sim::fmath`) and
//! each side checks the other's checksum as it goes.
//!
//! - [`wire`]: the datagrams.
//! - [`lockstep`]: the frames of both sides, which frame may run, the acknowledgements and the checksums, apart
//!   from any socket (tested over a simulated bad network).
//! - [`session`]: the host's port, the joiner's hello, the handshake that agrees the match and the input delay, and
//!   the running [`Session`].
//! - [`stun`]: this machine's public address, from a free public STUN server, and whether its router is a symmetric
//!   NAT.
//! - [`upnp`]: the host's router asked to forward the port (UPnP), so nobody has to set up port forwarding.
//!
//! The crate knows nothing of the game: a frame is the game's bytes (`qwc-game`'s `online` module packs the input),
//! and so are the joiner's request and the host's setup.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, UdpSocket};
use std::time::Duration;

pub mod lockstep;
pub mod session;
pub mod stun;
pub mod upnp;
pub mod wire;

pub use session::{
    delay_for, join, query, resolve, End, Error, Host, Joining, Knock, LobbyInfo, Request, Session, Welcomed,
    JOIN_WAIT, TICK,
};

/// The port the host listens on unless told another (UDP).
pub const DEFAULT_PORT: u16 = 20030;

/// This machine's IPv4 address on its network: the one its traffic to the internet leaves from (a socket is
/// pointed at a public address; nothing is sent).
pub fn lan_ipv4() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:53").ok()?;
    match s.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// This machine's IPv6 address on the internet, if it has one (global: not link-local or unique-local), found as
/// [`lan_ipv4`] is. Over IPv6 there is no address translation: a joiner reaches it if the firewalls let it.
pub fn global_ipv6() -> Option<Ipv6Addr> {
    let s = UdpSocket::bind("[::]:0").ok()?;
    s.connect("[2001:4860:4860::8888]:53").ok()?;
    match s.local_addr().ok()?.ip() {
        // 2000::/3 is the global unicast range
        IpAddr::V6(ip) if ip.segments()[0] & 0xe000 == 0x2000 => Some(ip),
        _ => None,
    }
}

/// What this machine's connection allows, found without changing anything: its addresses, its public IPv4 address
/// and whether its router is a symmetric NAT (`stun`), and whether the router takes UPnP requests (and its public
/// address), for the launcher's check.
#[derive(Clone, Debug)]
pub struct Probe {
    pub lan: Option<Ipv4Addr>,
    pub ipv6: Option<Ipv6Addr>,
    pub stun: stun::Mapping,
    /// The router's public address when it answered UPnP (none if it would not say), or why it did not.
    pub upnp: Result<Option<IpAddr>, String>,
}

/// [`Probe`]: a few seconds at most.
pub fn probe() -> Probe {
    let lan = lan_ipv4();
    let wait = Duration::from_millis(1500);
    let stun = UdpSocket::bind("0.0.0.0:0").map(|s| stun::public_address(&s, &stun::SERVERS, wait)).unwrap_or_default();
    let upnp = match lan {
        Some(ip) => upnp::Router::find(ip, Duration::from_secs(2)).map(|r| r.external_ip(wait)),
        None => Err("no network".into()),
    };
    Probe { lan, ipv6: global_ipv6(), stun, upnp }
}
