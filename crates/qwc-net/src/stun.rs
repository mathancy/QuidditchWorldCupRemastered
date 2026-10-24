//! This machine's address as the internet sees it, from a public STUN server (RFC 5389's binding request, answered
//! with the address and port the request came from): the host's address to give the joiner, and the joiner's to give
//! the host when its router needs a hole punched. Free public servers (Google's, Cloudflare's) answer; nothing of
//! ours runs anywhere. The request goes out on the game's own socket, so the port it reports is the one the router
//! maps for that socket.
//!
//! Two servers asked from the same socket report the same port behind most home routers ("cone" NATs, which a punched
//! hole gets through); a different port from each means a symmetric NAT, which only port forwarding, UPnP or a virtual
//! network gets through.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

/// Public STUN servers, asked in turn.
pub const SERVERS: [&str; 3] = ["stun.l.google.com:19302", "stun.cloudflare.com:3478", "stun1.l.google.com:19302"];

const MAGIC_COOKIE: u32 = 0x2112_a442;
const BINDING_REQUEST: u16 = 0x0001;
const BINDING_SUCCESS: u16 = 0x0101;
const MAPPED_ADDRESS: u16 = 0x0001;
const XOR_MAPPED_ADDRESS: u16 = 0x0020;

/// A binding request with transaction id `tid`.
pub fn request(tid: [u8; 12]) -> [u8; 20] {
    let mut b = [0u8; 20];
    b[0..2].copy_from_slice(&BINDING_REQUEST.to_be_bytes());
    // no attributes
    b[4..8].copy_from_slice(&MAGIC_COOKIE.to_be_bytes());
    b[8..20].copy_from_slice(&tid);
    b
}

/// The mapped address in a binding success for transaction `tid` (the XOR'd one, else the plain one).
pub fn parse_response(b: &[u8], tid: [u8; 12]) -> Option<SocketAddr> {
    if b.len() < 20 || u16::from_be_bytes([b[0], b[1]]) != BINDING_SUCCESS || b[4..8] != MAGIC_COOKIE.to_be_bytes() {
        return None;
    }
    if b[8..20] != tid {
        return None;
    }
    let len = usize::from(u16::from_be_bytes([b[2], b[3]]));
    let attrs = b.get(20..20 + len)?;
    let (mut xored, mut plain) = (None, None);
    let mut at = 0;
    while at + 4 <= attrs.len() {
        let kind = u16::from_be_bytes([attrs[at], attrs[at + 1]]);
        let n = usize::from(u16::from_be_bytes([attrs[at + 2], attrs[at + 3]]));
        let value = attrs.get(at + 4..at + 4 + n)?;
        match kind {
            XOR_MAPPED_ADDRESS => xored = address(value, Some(&b[4..20])),
            MAPPED_ADDRESS => plain = address(value, None),
            _ => {}
        }
        // attributes are padded to four bytes
        at += 4 + n.div_ceil(4) * 4;
    }
    xored.or(plain)
}

/// An address attribute's value: family, port, address; XOR'd with the cookie and the transaction id (`key`, the
/// header's bytes 4..20) when it is the XOR'd one.
fn address(v: &[u8], key: Option<&[u8]>) -> Option<SocketAddr> {
    let family = *v.get(1)?;
    let mut port = u16::from_be_bytes([*v.get(2)?, *v.get(3)?]);
    let mut ip = v.get(4..)?.to_vec();
    if let Some(key) = key {
        port ^= (MAGIC_COOKIE >> 16) as u16;
        ip.iter_mut().zip(key).for_each(|(b, k)| *b ^= k);
    }
    let ip = match family {
        1 => IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(ip.get(..4)?).ok()?)),
        2 => IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(ip.get(..16)?).ok()?)),
        _ => return None,
    };
    Some(SocketAddr::new(ip, port))
}

/// A transaction id: the clock and a counter mixed (it only has to match its answer).
fn transaction() -> [u8; 12] {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() as u64;
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut x = t ^ n.rotate_left(32) ^ u64::from(std::process::id());
    let mut out = [0u8; 12];
    for chunk in out.chunks_mut(4) {
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
        chunk.copy_from_slice(&(x as u32).to_le_bytes());
    }
    out
}

/// What the servers said of `socket`'s public address.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mapping {
    /// The address the first server to answer saw.
    pub public: Option<SocketAddr>,
    /// Whether a second server saw another port (a symmetric NAT: punched holes do not get through it).
    pub symmetric: bool,
}

/// Asks up to two of `servers` (in `SERVERS`' form) for `socket`'s public IPv4 address (IPv6 has no address
/// translation to find out about), each within `wait`; from an IPv6 socket that takes IPv4 too (Linux's) the servers
/// are reached at their IPv4-mapped addresses. The socket's own datagrams that come meanwhile are dropped, so ask
/// before playing. Its read timeout is left changed.
pub fn public_address(socket: &UdpSocket, servers: &[&str], wait: Duration) -> Mapping {
    let v6 = socket.local_addr().is_ok_and(|a| a.is_ipv6());
    let mut seen: Vec<SocketAddr> = Vec::new();
    for server in servers {
        if seen.len() == 2 {
            break;
        }
        let Some(to) = server.to_socket_addrs().ok().and_then(|mut a| a.find(SocketAddr::is_ipv4)) else {
            continue;
        };
        let to = match to {
            SocketAddr::V4(a) if v6 => SocketAddr::new(IpAddr::V6(a.ip().to_ipv6_mapped()), a.port()),
            to => to,
        };
        let tid = transaction();
        let until = Instant::now() + wait;
        let mut sent = None::<Instant>;
        let mut buf = [0u8; 576];
        while Instant::now() < until {
            // said again every 300 ms (UDP may lose it)
            if sent.is_none_or(|t| t.elapsed() >= Duration::from_millis(300)) {
                let _ = socket.send_to(&request(tid), to);
                sent = Some(Instant::now());
            }
            let _ = socket.set_read_timeout(Some(Duration::from_millis(50)));
            match socket.recv_from(&mut buf) {
                Ok((n, from)) if from == to => {
                    if let Some(a) = parse_response(&buf[..n], tid) {
                        seen.push(a);
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    Mapping { public: seen.first().copied(), symmetric: seen.len() == 2 && seen[0].port() != seen[1].port() }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 5769's sample IPv4 response (2.2), its software, integrity and fingerprint attributes skipped over.
    #[test]
    fn reads_the_rfc_sample_response() {
        let tid = [0xb7, 0xe7, 0xa7, 0x01, 0xbc, 0x34, 0xd6, 0x86, 0xfa, 0x87, 0xdf, 0xae];
        let b: Vec<u8> = [
            "0101003c2112a442b7e7a701bc34d686fa87dfae",
            "8022000b7465737420766563746f7220",
            "00200008",
            "0001a147e112a643",
            "000800149ae35fdd8aa4bd0d3a6ebb6b9b2c23e7cd0aeb8e",
            "80280004c07d4c96",
        ]
        .concat()
        .as_bytes()
        .chunks(2)
        .map(|h| u8::from_str_radix(std::str::from_utf8(h).unwrap(), 16).unwrap())
        .collect();
        assert_eq!(parse_response(&b, tid), Some("192.0.2.1:32853".parse().unwrap()));
        let mut other = tid;
        other[0] ^= 1;
        assert_eq!(parse_response(&b, other), None, "another transaction's answer");
        assert_eq!(parse_response(&b[..30], tid), None, "cut short");
    }

    #[test]
    fn a_request_is_a_bare_binding_request() {
        let r = request([7; 12]);
        assert_eq!(&r[..4], &[0, 1, 0, 0]);
        assert_eq!(&r[4..8], &MAGIC_COOKIE.to_be_bytes());
        assert_eq!(&r[8..], &[7; 12]);
    }

    /// A server on this machine that answers with the plain mapped address: the client reads it.
    #[test]
    fn asks_a_server_for_the_address() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let at = server.local_addr().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            let (n, from) = server.recv_from(&mut buf).unwrap();
            assert_eq!(n, 20);
            let mut r = vec![0x01, 0x01, 0x00, 0x0c];
            r.extend_from_slice(&buf[4..20]);
            r.extend_from_slice(&[0x00, 0x01, 0x00, 0x08, 0x00, 0x01]);
            r.extend_from_slice(&from.port().to_be_bytes());
            r.extend_from_slice(&[127, 0, 0, 1]);
            server.send_to(&r, from).unwrap();
        });
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        let m = public_address(&client, &[&at.to_string()], Duration::from_secs(2));
        assert_eq!(m.public, Some(client.local_addr().unwrap()));
        assert!(!m.symmetric);
    }
}
