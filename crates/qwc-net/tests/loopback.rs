//! Host and joiner on this machine, over UDP through a proxy that loses and delays datagrams: the handshake agrees,
//! both run the same frames with the same two inputs, and the refusals and the leaving are seen.

use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use qwc_net::{End, Error, Host, Knock, Session};

const BUILD: &str = "test";

/// A UDP relay between the joiner (whoever writes first) and `host`, losing `loss` of the datagrams and delaying the
/// rest by 20-40 ms. Stops when `stop` is set.
fn proxy(host: SocketAddr, loss: u32, stop: Arc<AtomicBool>) -> SocketAddr {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = socket.local_addr().unwrap();
    socket.set_read_timeout(Some(Duration::from_millis(2))).unwrap();
    thread::spawn(move || {
        let mut joiner = None;
        let mut queue: Vec<(Instant, Vec<u8>, SocketAddr)> = Vec::new();
        let mut rng = 777u32;
        let mut buf = [0u8; 2048];
        while !stop.load(Ordering::Relaxed) {
            if let Ok((n, from)) = socket.recv_from(&mut buf) {
                let to = if from == host {
                    joiner
                } else {
                    joiner = Some(from);
                    Some(host)
                };
                rng = rng.wrapping_mul(1_103_515_245).wrapping_add(12345);
                let roll = (rng >> 16) % 100;
                if let Some(to) = to.filter(|_| roll >= loss) {
                    let late = Duration::from_millis(20 + u64::from(roll % 21));
                    queue.push((Instant::now() + late, buf[..n].to_vec(), to));
                }
            }
            let now = Instant::now();
            queue.retain(|(due, b, to)| {
                if *due <= now {
                    let _ = socket.send_to(b, to);
                    false
                } else {
                    true
                }
            });
        }
    });
    addr
}

fn localhost(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// Runs a side for `frames` frames as fast as the inputs come: answers the pairs it ran.
fn play(s: &mut Session, frames: u32) -> Vec<[Vec<u8>; 2]> {
    let mut ran = Vec::new();
    let began = Instant::now();
    // the frames given so far: the first is frame `delay`
    let mut given = s.delay();
    while (ran.len() as u32) < frames {
        assert!(began.elapsed() < Duration::from_secs(60), "stuck at frame {}", s.frame());
        s.poll();
        assert_eq!(s.end(), None);
        // as the game does: an input, then the frame it lets run
        if s.wants_input() {
            s.input(vec![s.seat() as u8, given as u8, (given >> 8) as u8]);
            given += 1;
        }
        if let Some(pair) = s.ready() {
            ran.push(pair.clone());
            // a checksum both sides compute alike
            s.ran(ran.len() as u64 * 31 + u64::from(pair[0][1]) + u64::from(pair[1][1]));
        } else {
            thread::sleep(Duration::from_millis(1));
        }
    }
    // a little longer, so the last frames and checksums reach the other side
    let until = Instant::now() + Duration::from_millis(300);
    while Instant::now() < until {
        s.poll();
        thread::sleep(Duration::from_millis(5));
    }
    ran
}

fn host_side(
    host: Host,
    start_sum: u64,
    then: impl FnOnce(Session) -> Vec<[Vec<u8>; 2]> + Send + 'static,
) -> thread::JoinHandle<Result<Vec<[Vec<u8>; 2]>, Error>> {
    let mut host = host;
    thread::spawn(move || {
        let r = loop {
            match host.poll(Duration::from_secs(10)).unwrap() {
                Some(Knock::Join(r)) => break r,
                Some(Knock::OtherBuild { .. }) => continue,
                None => panic!("nobody came"),
            }
        };
        assert_eq!(r.ask, b"France");
        let s = host.accept(r, b"England v France at Wales", start_sum, 3, None)?;
        Ok(then(s))
    })
}

#[test]
fn both_sides_run_the_same_frames_through_a_lossy_link() {
    let stop = Arc::new(AtomicBool::new(false));
    let host = Host::bind(0, BUILD).unwrap();
    let via = proxy(localhost(host.port()), 15, stop.clone());
    let h = host_side(host, 42, |mut s| {
        assert_eq!(s.seat(), 0);
        play(&mut s, 200)
    });
    let mut got_setup = Vec::new();
    let mut s = qwc_net::join(&via.to_string(), 1, BUILD, b"France", 3, |setup| {
        got_setup = setup.to_vec();
        Ok(42)
    })
    .unwrap();
    assert_eq!(got_setup, b"England v France at Wales");
    assert_eq!(s.seat(), 1);
    // 40-80 ms round trips: two or three frames of delay
    assert!((2..=3).contains(&s.delay()), "delay {}", s.delay());
    let joiner = play(&mut s, 200);
    let host = h.join().unwrap().unwrap();
    stop.store(true, Ordering::Relaxed);
    assert_eq!(host[..200], joiner[..200]);
    let d = s.delay() as usize;
    for (k, pair) in joiner.iter().enumerate().skip(d) {
        assert_eq!(pair[0], vec![0, k as u8, (k >> 8) as u8], "the host's input for frame {k}");
        assert_eq!(pair[1], vec![1, k as u8, (k >> 8) as u8], "the joiner's input for frame {k}");
    }
    assert!(s.rtt().is_some_and(|r| r >= Duration::from_millis(30)), "{:?}", s.rtt());
}

#[test]
fn different_data_is_refused_on_both_sides() {
    let host = Host::bind(0, BUILD).unwrap();
    let port = host.port();
    let h = host_side(host, 42, |_| Vec::new());
    let r = qwc_net::join(&format!("127.0.0.1:{port}"), 1, BUILD, b"France", 3, |_| Ok(43));
    assert!(matches!(r, Err(Error::DataDiffers)), "{:?}", r.err());
    assert!(matches!(h.join().unwrap(), Err(Error::DataDiffers)));
}

#[test]
fn another_build_is_turned_away() {
    let mut host = Host::bind(0, BUILD).unwrap();
    let port = host.port();
    let j = thread::spawn(move || qwc_net::join(&format!("127.0.0.1:{port}"), 1, "other", b"", 3, |_| Ok(0)));
    let knock = host.poll(Duration::from_secs(5)).unwrap();
    assert!(matches!(knock, Some(Knock::OtherBuild { ref build, .. }) if build == "other"), "{knock:?}");
    match j.join().unwrap() {
        Err(Error::Refused(why)) => assert!(why.contains("version test"), "{why}"),
        r => panic!("{:?}", r.err()),
    }
}

#[test]
fn leaving_is_seen_by_the_other_side() {
    let host = Host::bind(0, BUILD).unwrap();
    let port = host.port();
    let h = host_side(host, 1, |mut s| {
        play(&mut s, 20);
        let began = Instant::now();
        while s.end().is_none() && began.elapsed() < Duration::from_secs(5) {
            s.poll();
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(s.end(), Some(&End::Left("back to the menu".into())));
        Vec::new()
    });
    let mut s = qwc_net::join(&format!("127.0.0.1:{port}"), 1, BUILD, b"France", 3, |_| Ok(1)).unwrap();
    play(&mut s, 20);
    s.close("back to the menu");
    assert_eq!(s.end(), Some(&End::Closed));
    h.join().unwrap().unwrap();
}

/// The check before joining: a waiting host answers with its build and what it offers; a port with no host
/// times out; the query does not take the host's place for a joiner.
#[test]
fn a_query_finds_a_waiting_host() {
    let mut host = Host::bind(0, BUILD).unwrap();
    host.set_about(b"team England");
    let port = host.port();
    let h = thread::spawn(move || {
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(3) {
            if let Some(k) = host.poll(Duration::from_millis(100)).unwrap() {
                return Some(k);
            }
        }
        None
    });
    let info = qwc_net::query(&format!("127.0.0.1:{port}"), 1, Duration::from_secs(2)).unwrap();
    assert_eq!((info.build.as_str(), info.about.as_slice()), (BUILD, &b"team England"[..]));
    assert!(info.rtt < Duration::from_secs(1));
    assert!(h.join().unwrap().is_none(), "a query is no knock");
    let r = qwc_net::query(&format!("127.0.0.1:{port}"), 1, Duration::from_millis(600));
    assert!(matches!(r, Err(Error::TimedOut(_))), "{:?}", r.err());
}

#[test]
fn nobody_hosting_times_out() {
    // a port nobody listens on (bound, then closed)
    let port = UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let began = Instant::now();
    let j = qwc_net::Joining::bind(&format!("127.0.0.1:{port}"), 1).unwrap();
    let r = j.hello(BUILD, b"", Duration::from_secs(2));
    assert!(matches!(r, Err(Error::TimedOut(_))), "{:?}", r.err());
    assert!(began.elapsed() >= Duration::from_secs(2));
}

/// A host behind a router that drops what comes first (as a NAT does) lets the joiner in by sending to it: the
/// joiner's hellos then get through.
#[test]
fn a_host_lets_a_joiner_in() {
    let host = Host::bind(0, BUILD).unwrap();
    let port = host.port();
    // the "router": joiner's datagrams to the host are dropped until the host has sent to the joiner
    let stop = Arc::new(AtomicBool::new(false));
    let router = UdpSocket::bind("127.0.0.1:0").unwrap();
    let via = router.local_addr().unwrap();
    router.set_read_timeout(Some(Duration::from_millis(5))).unwrap();
    let j = qwc_net::Joining::bind(&via.to_string(), 1).unwrap();
    let (s, open) = (stop.clone(), Arc::new(AtomicBool::new(false)));
    let o = open.clone();
    thread::spawn(move || {
        let mut joiner = None;
        let mut buf = [0u8; 2048];
        while !s.load(Ordering::Relaxed) {
            let Ok((n, from)) = router.recv_from(&mut buf) else { continue };
            if from.port() == port {
                // the host's datagram opens the way (and goes on to the joiner)
                o.store(true, Ordering::Relaxed);
                if let Some(to) = joiner {
                    let _ = router.send_to(&buf[..n], to);
                }
            } else {
                joiner = Some(from);
                if o.load(Ordering::Relaxed) {
                    let _ = router.send_to(&buf[..n], localhost(port));
                }
            }
        }
    });
    let h = thread::spawn(move || {
        let mut host = host;
        let began = Instant::now();
        loop {
            assert!(began.elapsed() < Duration::from_secs(10), "the joiner never got in");
            // the joiner's public address as the host is told it: here, the router's side facing the host
            host.punch(via);
            if let Some(Knock::Join(r)) = host.poll(Duration::from_millis(200)).unwrap() {
                return host.accept(r, b"setup", 5, 3, Some(1)).map(|s| s.seat());
            }
        }
    });
    let setup = j.hello(BUILD, b"", Duration::from_secs(10)).unwrap();
    assert_eq!(setup, b"setup");
    assert!(open.load(Ordering::Relaxed));
    let s = j.ready(5, 3).unwrap();
    assert_eq!((s.seat(), h.join().unwrap().unwrap()), (1, 0));
    stop.store(true, Ordering::Relaxed);
}
