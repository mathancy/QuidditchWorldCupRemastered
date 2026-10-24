//! What this machine's connection allows for hosting, found without changing anything (`qwc_net::probe`):
//! `cargo run -p qwc-net --example probe [ADDRESS]`; with an address, also whether a host waits there
//! (`qwc_net::query`, the launcher's Check).

fn main() {
    if let Some(to) = std::env::args().nth(1) {
        match qwc_net::query(&to, qwc_net::DEFAULT_PORT, std::time::Duration::from_secs(3)) {
            Ok(i) => {
                println!("host at {}: build {}, {:?}, {:?}", i.host, i.build, String::from_utf8_lossy(&i.about), i.rtt)
            }
            Err(e) => println!("no host at {to}: {e}"),
        }
    }
    let p = qwc_net::probe();
    println!("address on the network: {:?}", p.lan);
    println!("IPv6 on the internet:   {:?}", p.ipv6);
    println!("public address (STUN):  {:?}, symmetric NAT: {}", p.stun.public, p.stun.symmetric);
    match p.upnp {
        Ok(ip) => println!("UPnP router: yes, its public address {ip:?}"),
        Err(e) => println!("UPnP router: no ({e})"),
    }
}
