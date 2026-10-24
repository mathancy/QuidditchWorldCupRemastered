//! The router asked to forward the host's port (UPnP's Internet Gateway Device protocol, as consoles and many games
//! do), so a joiner over the internet reaches the host without anyone setting up port forwarding by hand. Many home
//! routers allow it; some have it turned off, and then this says so and the other ways remain (`docs/ONLINE.md`).
//!
//! The steps: an SSDP search on the local network (a multicast datagram to 239.255.255.250:1900) finds the router's
//! description; its WANIPConnection (or WANPPPConnection) service's control URL takes SOAP requests over HTTP:
//! `AddPortMapping` (the host's UDP port to this machine, for a few hours or until removed), `GetExternalIPAddress`
//! and `DeletePortMapping` when the game closes ([`Forwarded`]'s drop). Plain `std`: a small HTTP/1.1 client and
//! string searches over the XML, which is all the protocol needs.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

/// How long a mapping is asked for (the game removes it on closing; a crash leaves it to expire).
const LEASE_SECONDS: u32 = 4 * 3600;
const DESCRIPTION: &str = "QWC Remastered";
const SSDP: &str = "239.255.255.250:1900";
/// The services that map ports, newest first.
const SERVICES: [&str; 3] = [
    "urn:schemas-upnp-org:service:WANIPConnection:2",
    "urn:schemas-upnp-org:service:WANIPConnection:1",
    "urn:schemas-upnp-org:service:WANPPPConnection:1",
];

/// An `http://host[:port]/path` URL's parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Url {
    pub host: String,
    pub port: u16,
    pub path: String,
}

impl Url {
    pub fn parse(url: &str) -> Option<Url> {
        let rest = url.trim().strip_prefix("http://")?;
        let (authority, path) = rest.split_once('/').map_or((rest, "/".to_string()), |(a, p)| (a, format!("/{p}")));
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) if !h.contains(']') || h.ends_with(']') => (h, p.parse().ok()?),
            _ => (authority, 80),
        };
        Some(Url { host: host.trim_matches(['[', ']']).to_string(), port, path })
    }

    /// `relative` (an absolute URL, or a path) against this one.
    pub fn join(&self, relative: &str) -> Option<Url> {
        let r = relative.trim();
        if r.starts_with("http://") {
            return Url::parse(r);
        }
        let path = if r.starts_with('/') { r.to_string() } else { format!("/{r}") };
        Some(Url { path, ..self.clone() })
    }

    fn addr(&self) -> Option<SocketAddr> {
        (self.host.as_str(), self.port).to_socket_addrs().ok()?.next()
    }
}

/// The text between the first `<name>` (or `<prefix:name>`) and its closing tag in `xml`.
pub fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let mut at = 0;
    while let Some(k) = xml[at..].find('<') {
        let start = at + k + 1;
        let end = start + xml[start..].find('>')?;
        let open = &xml[start..end];
        let bare = open.split_whitespace().next().unwrap_or("");
        let local = bare.rsplit(':').next().unwrap_or(bare);
        if local == name && !open.starts_with('/') && !open.ends_with('/') {
            let close = format!("</{bare}>");
            let body_end = end + 1 + xml[end + 1..].find(&close)?;
            return Some(xml[end + 1..body_end].trim());
        }
        at = end;
    }
    None
}

/// The port-mapping service in a device description: its type and control URL (joined to `base`, the description's
/// own URL or its `URLBase`).
pub fn find_service(description: &str, base: &Url) -> Option<(String, Url)> {
    let base = tag(description, "URLBase").and_then(Url::parse).unwrap_or_else(|| base.clone());
    let mut found: Vec<(usize, String, Url)> = Vec::new();
    let mut rest = description;
    while let Some(k) = rest.find("<service>") {
        let block_end = rest[k..].find("</service>").map_or(rest.len(), |e| k + e);
        let block = &rest[k..block_end];
        if let (Some(kind), Some(control)) = (tag(block, "serviceType"), tag(block, "controlURL")) {
            if let (Some(rank), Some(url)) = (SERVICES.iter().position(|s| *s == kind), base.join(control)) {
                found.push((rank, kind.to_string(), url));
            }
        }
        rest = &rest[block_end..];
    }
    found.sort_by_key(|f| f.0);
    found.into_iter().next().map(|(_, kind, url)| (kind, url))
}

/// An HTTP/1.1 request to `url` (`Connection: close`): the status and the body (de-chunked).
fn http(url: &Url, method: &str, headers: &str, body: &str, wait: Duration) -> Result<(u16, String), String> {
    let addr = url.addr().ok_or_else(|| format!("no address for {}", url.host))?;
    let mut s = TcpStream::connect_timeout(&addr, wait).map_err(|e| format!("{addr}: {e}"))?;
    s.set_read_timeout(Some(wait)).map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(wait)).map_err(|e| e.to_string())?;
    let req = format!(
        "{method} {} HTTP/1.1\r\nHost: {}:{}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        url.path,
        url.host,
        url.port,
        body.len()
    );
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    // a router may keep the connection open past its answer: what came by the timeout is the answer
    let _ = s.read_to_end(&mut raw);
    parse_http(&raw).ok_or_else(|| format!("{addr}: not an HTTP answer"))
}

/// An HTTP response's status and body.
pub fn parse_http(raw: &[u8]) -> Option<(u16, String)> {
    let text = String::from_utf8_lossy(raw);
    let (head, body) = text.split_once("\r\n\r\n")?;
    let status = head.split_whitespace().nth(1)?.parse().ok()?;
    let chunked = head.lines().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("transfer-encoding:") && l.contains("chunked")
    });
    if !chunked {
        return Some((status, body.to_string()));
    }
    let mut out = String::new();
    let mut rest = body;
    while let Some((size, after)) = rest.split_once("\r\n") {
        let n = usize::from_str_radix(size.split(';').next()?.trim(), 16).ok()?;
        if n == 0 {
            break;
        }
        out.push_str(after.get(..n)?);
        rest = after.get(n..)?.trim_start_matches("\r\n");
    }
    Some((status, out))
}

/// A SOAP action on the service: the answer's body, or the router's error (its `errorCode` and description).
fn soap(control: &Url, service: &str, action: &str, args: &[(&str, String)], wait: Duration) -> Result<String, String> {
    let args: String = args.iter().map(|(k, v)| format!("<{k}>{v}</{k}>")).collect();
    let body = format!(
        "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\" \
         s:encodingStyle=\"http://schemas.xmlsoap.org/soap/encoding/\"><s:Body><u:{action} xmlns:u=\"{service}\">\
         {args}</u:{action}></s:Body></s:Envelope>"
    );
    let headers = format!("Content-Type: text/xml; charset=\"utf-8\"\r\nSOAPAction: \"{service}#{action}\"\r\n");
    let (status, answer) = http(control, "POST", &headers, &body, wait)?;
    if status == 200 {
        return Ok(answer);
    }
    let code = tag(&answer, "errorCode").unwrap_or("?");
    let what = tag(&answer, "errorDescription").unwrap_or("");
    Err(format!("{action}: the router answered {status} (error {code} {what})").trim().to_string())
}

/// The routers' descriptions on the network: the `LOCATION`s of the SSDP answers within `wait` (a moment after the
/// first, for a second router), searched from `local` (this machine's address on the network, so the search goes out
/// where the router is).
pub fn discover(local: Ipv4Addr, wait: Duration) -> Vec<Url> {
    let Ok(socket) = UdpSocket::bind((local, 0)) else { return Vec::new() };
    let mut found = Vec::new();
    let mut until = Instant::now() + wait;
    let mut buf = [0u8; 2048];
    let mut sent = None::<Instant>;
    while Instant::now() < until {
        if sent.is_none_or(|t| t.elapsed() >= Duration::from_millis(700)) {
            for st in ["urn:schemas-upnp-org:device:InternetGatewayDevice:1", SERVICES[1], SERVICES[2]] {
                let search = format!(
                    "M-SEARCH * HTTP/1.1
HOST: {SSDP}
ST: {st}
MAN: \"ssdp:discover\"
MX: 2

"
                );
                let _ = socket.send_to(search.as_bytes(), SSDP);
            }
            sent = Some(Instant::now());
        }
        let _ = socket.set_read_timeout(Some(Duration::from_millis(50)));
        let Ok((n, _)) = socket.recv_from(&mut buf) else { continue };
        let text = String::from_utf8_lossy(&buf[..n]);
        let location = text.lines().find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.trim().eq_ignore_ascii_case("location").then(|| v.trim().to_string())
        });
        if let Some(url) = location.as_deref().and_then(Url::parse) {
            if !found.contains(&url) {
                found.push(url);
            }
            until = until.min(Instant::now() + Duration::from_millis(300));
        }
    }
    found
}

/// A port the router forwards to this machine, until dropped.
#[derive(Debug)]
pub struct Forwarded {
    control: Url,
    service: String,
    pub port: u16,
    /// The router's address on the internet, if it said.
    pub external_ip: Option<IpAddr>,
}

impl Forwarded {
    /// Asks the router to stop forwarding the port.
    pub fn remove(&self) -> Result<(), String> {
        let args = [
            ("NewRemoteHost", String::new()),
            ("NewExternalPort", self.port.to_string()),
            ("NewProtocol", "UDP".to_string()),
        ];
        soap(&self.control, &self.service, "DeletePortMapping", &args, Duration::from_secs(2)).map(|_| ())
    }
}

impl Drop for Forwarded {
    fn drop(&mut self) {
        let _ = self.remove();
    }
}

/// A router's port-mapping service.
#[derive(Clone, Debug)]
pub struct Router {
    /// Its description's URL, and the service's type and control URL.
    pub location: Url,
    pub service: String,
    control: Url,
}

impl Router {
    /// The router whose description is at `location`.
    pub fn at(location: &Url, wait: Duration) -> Result<Router, String> {
        let (status, description) = http(location, "GET", "", "", wait)?;
        if status != 200 {
            return Err(format!("the router's description: HTTP {status}"));
        }
        let (service, control) =
            find_service(&description, location).ok_or("the router maps no ports (no WAN service)")?;
        Ok(Router { location: location.clone(), service, control })
    }

    /// The router on the network, searched from `local` (this machine's address on it).
    pub fn find(local: Ipv4Addr, wait: Duration) -> Result<Router, String> {
        let routers = discover(local, wait);
        if routers.is_empty() {
            return Err("no router answered (UPnP is off, or the network has none)".into());
        }
        let mut last = String::new();
        for r in &routers {
            match Router::at(r, wait) {
                Ok(r) => return Ok(r),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    /// The router's address on the internet.
    pub fn external_ip(&self, wait: Duration) -> Option<IpAddr> {
        let a = soap(&self.control, &self.service, "GetExternalIPAddress", &[], wait).ok()?;
        tag(&a, "NewExternalIPAddress")?.parse().ok()
    }

    /// Asks the router to forward UDP `port` to `local:port` (until the answer is dropped).
    pub fn forward(&self, local: Ipv4Addr, port: u16, wait: Duration) -> Result<Forwarded, String> {
        let add = |lease: u32| {
            let args = [
                ("NewRemoteHost", String::new()),
                ("NewExternalPort", port.to_string()),
                ("NewProtocol", "UDP".to_string()),
                ("NewInternalPort", port.to_string()),
                ("NewInternalClient", local.to_string()),
                ("NewEnabled", "1".to_string()),
                ("NewPortMappingDescription", DESCRIPTION.to_string()),
                ("NewLeaseDuration", lease.to_string()),
            ];
            soap(&self.control, &self.service, "AddPortMapping", &args, wait)
        };
        // error 725: the router takes only permanent mappings (lease 0), removed on closing all the same
        if let Err(e) = add(LEASE_SECONDS) {
            if !e.contains("error 725") {
                return Err(e);
            }
            add(0)?;
        }
        let external_ip = self.external_ip(wait);
        Ok(Forwarded { control: self.control.clone(), service: self.service.clone(), port, external_ip })
    }
}

/// Finds the router and asks it to forward UDP `port` to this machine (`local`, its address on the network).
pub fn forward(local: Ipv4Addr, port: u16, wait: Duration) -> Result<Forwarded, String> {
    Router::find(local, wait)?.forward(local, port, wait)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    #[test]
    fn urls_and_their_paths() {
        let u = Url::parse("http://192.168.1.1:5000/rootDesc.xml").unwrap();
        assert_eq!(u, Url { host: "192.168.1.1".into(), port: 5000, path: "/rootDesc.xml".into() });
        assert_eq!(Url::parse("http://router/").unwrap().port, 80);
        assert_eq!(u.join("ctl/IPConn").unwrap().path, "/ctl/IPConn");
        assert_eq!(u.join("/upnp/control").unwrap().path, "/upnp/control");
        assert_eq!(u.join("http://10.0.0.1:80/x").unwrap().host, "10.0.0.1");
        assert_eq!(Url::parse("https://x/"), None);
    }

    const DESCRIPTION_XML: &str = r#"<?xml version="1.0"?>
<root xmlns="urn:schemas-upnp-org:device-1-0"><device><deviceType>urn:schemas-upnp-org:device:InternetGatewayDevice:1</deviceType>
<serviceList><service><serviceType>urn:schemas-upnp-org:service:Layer3Forwarding:1</serviceType><controlURL>/ctl/L3F</controlURL></service></serviceList>
<deviceList><device><deviceList><device><serviceList>
<service>
  <serviceType>urn:schemas-upnp-org:service:WANPPPConnection:1</serviceType>
  <controlURL>/ctl/PPPConn</controlURL>
</service>
<service>
  <serviceType>urn:schemas-upnp-org:service:WANIPConnection:1</serviceType>
  <serviceId>urn:upnp-org:serviceId:WANIPConn1</serviceId>
  <controlURL>/ctl/IPConn</controlURL>
</service>
</serviceList></device></deviceList></device></deviceList></device></root>"#;

    #[test]
    fn the_ip_connection_is_found_in_a_description() {
        let base = Url::parse("http://192.168.1.1:5000/rootDesc.xml").unwrap();
        let (kind, url) = find_service(DESCRIPTION_XML, &base).unwrap();
        assert_eq!(kind, SERVICES[1]);
        assert_eq!(url.path, "/ctl/IPConn");
        assert_eq!(tag("<a><s:b x=\"1\">hi</s:b></a>", "b"), Some("hi"));
        assert_eq!(tag("<a><b/></a>", "b"), None);
    }

    #[test]
    fn chunked_answers_read_whole() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        assert_eq!(parse_http(raw), Some((200, "hello world".into())));
        assert_eq!(parse_http(b"HTTP/1.0 500 Internal\r\n\r\nx"), Some((500, "x".into())));
    }

    /// A router on this machine: it serves the description and answers the SOAP actions, recording them; the first
    /// AddPortMapping is refused with 725 (permanent mappings only).
    fn fake_router(actions: Arc<Mutex<Vec<String>>>) -> Url {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in l.incoming() {
                let mut s = stream.unwrap();
                s.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
                let mut req = Vec::new();
                let mut buf = [0u8; 4096];
                // read the head and the body its length gives
                loop {
                    let n = s.read(&mut buf).unwrap_or(0);
                    req.extend_from_slice(&buf[..n]);
                    let t = String::from_utf8_lossy(&req).to_string();
                    if let Some((head, body)) = t.split_once("\r\n\r\n") {
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("Content-Length: ").and_then(|v| v.trim().parse().ok()))
                            .unwrap_or(0);
                        if body.len() >= len {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                let t = String::from_utf8_lossy(&req).to_string();
                let (status, body) = if t.starts_with("GET /desc.xml") {
                    (200, DESCRIPTION_XML.to_string())
                } else if let Some(action) = t.lines().find_map(|l| l.strip_prefix("SOAPAction: ")) {
                    let action = action.trim_matches('"').rsplit('#').next().unwrap().to_string();
                    let mut a = actions.lock().unwrap();
                    a.push(format!("{action} {}", tag(&t, "NewLeaseDuration").unwrap_or("-")));
                    match action.as_str() {
                        "AddPortMapping" if a.len() == 1 => (500, "<errorCode>725</errorCode>".to_string()),
                        "GetExternalIPAddress" => {
                            (200, "<NewExternalIPAddress>203.0.113.7</NewExternalIPAddress>".to_string())
                        }
                        _ => (200, String::new()),
                    }
                } else {
                    (404, String::new())
                };
                let answer = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n\r\n{body}", body.len());
                let _ = s.write_all(answer.as_bytes());
            }
        });
        Url::parse(&format!("http://127.0.0.1:{port}/desc.xml")).unwrap()
    }

    #[test]
    fn a_router_forwards_and_forgets_the_port() {
        let actions = Arc::new(Mutex::new(Vec::new()));
        let at = fake_router(actions.clone());
        let router = Router::at(&at, Duration::from_secs(2)).unwrap();
        assert_eq!(router.service, SERVICES[1]);
        let f = router.forward(Ipv4Addr::new(192, 168, 1, 20), 20030, Duration::from_secs(2)).unwrap();
        assert_eq!(f.external_ip, Some("203.0.113.7".parse().unwrap()));
        drop(f);
        assert_eq!(
            *actions.lock().unwrap(),
            ["AddPortMapping 14400", "AddPortMapping 0", "GetExternalIPAddress -", "DeletePortMapping -"]
        );
    }
}
