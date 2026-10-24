//! Two copies of a real match fed the same inputs stay the same on every tick (what online play rests on,
//! `qwc-net`), and one different button press shows in the checksum on that tick. Runs only when `QWC_DATA_DIR`
//! points at a repo root holding `original/cd_files/`; otherwise it is a no-op.
//!
//! It prints a digest of every tick's checksum: run it on two platforms (Windows and Linux) and the digests must
//! match. `QWC_EXPECT_DIGEST=<hex>` makes the test check it.

use std::path::PathBuf;

use qwc_assets::Vfs;
use qwc_sim::human::Input;
use qwc_sim::race::SeekerPad;
use qwc_sim::{Console, Match};

fn real_match(vfs: &Vfs) -> Match {
    let mut c = Console::new();
    c.run_script(&vfs.read("root/CLI/autoexec.cli").unwrap());
    for (slot, team) in ["England", "Bulgaria"].iter().enumerate() {
        c.team = slot;
        c.run_script(&vfs.read(&format!("teams/{team}/team_3.cli")).unwrap());
    }
    let db = |p: &str| qwc_formats::formotion::parse(&vfs.read(p).unwrap()).unwrap();
    let (global, england, bulgaria) =
        (db("match/quidd/formotionDB.txt"), db("teams/England/formotionDB.txt"), db("teams/Bulgaria/formotionDB.txt"));
    let mut m = Match::new(&c, qwc_sim::Rand::new(7));
    m.use_formotions(&global, [Some(&england), Some(&bulgaria)]);
    let q = qwc_formats::qsp::Qsp::parse(&vfs.read("stadium/england/gfx/ENG0003.QSP").unwrap()).unwrap();
    m.use_spline(qwc_sim::spline::Spline::from_qsp(&q.splines[0], m.race_rules.seeker.spline_base_scale as f64));
    m.power.unlocked = [true, true, true, false];
    m.set_human(0, true);
    m.set_human(1, true);
    m
}

/// A player's hands: the stick swung about and the buttons pressed now and then, from their own generator (not the
/// match's).
struct Hands(u32);

impl Hands {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12345);
        self.0 >> 8
    }

    fn input(&mut self, tick: u32) -> (Input, SeekerPad) {
        let angle = f64::from(tick % 240) / 240.0 * std::f64::consts::TAU + f64::from(self.next() % 100) / 400.0;
        // the same bits on every platform, as the match's own
        let (s, c) = qwc_sim::fmath::sin_cos(angle);
        let mut press = |odds: u32| self.next().is_multiple_of(odds);
        let (action, shoot, boost, dodge) = (press(30), press(50), press(8), press(90));
        let i = Input {
            stick: [c, 0.0, s],
            raw: [c, 0.0, -s],
            action_pressed: action,
            action_held: action,
            shoot_pressed: shoot,
            shoot_held: shoot,
            boost_pressed: boost,
            boost_held: boost,
            dodge_pressed: dodge,
            ..Input::default()
        };
        let pad = SeekerPad { stick: [s, c], boost_held: boost, boost_pressed: boost, grab_held: press(20) };
        (i, pad)
    }
}

fn feed(m: &mut Match, hands: &mut [Hands; 2]) {
    for (t, h) in hands.iter_mut().enumerate() {
        let (i, pad) = h.input(m.tick);
        m.set_input(t, i);
        m.set_seeker_pad(t, pad);
    }
}

#[test]
fn two_copies_agree_on_every_tick() {
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let vfs = Vfs::open(root.join("original/cd_files")).unwrap();
    let (mut a, mut b) = (real_match(&vfs), real_match(&vfs));
    assert_eq!(a.checksum(), b.checksum(), "the same start");
    let (mut ha, mut hb) = ([Hands(1), Hands(2)], [Hands(1), Hands(2)]);
    // four minutes of the match, then the Seeker game (started there: the random hands seldom fill the bars)
    let mut digest = 0xcbf2_9ce4_8422_2325u64;
    let mut timing = std::time::Duration::ZERO;
    // `QWC_TRACE=FILE`: every tick's checksum to FILE (and the state at tick `QWC_TRACE_DUMP`), to find where two
    // platforms part
    let mut trace = std::env::var_os("QWC_TRACE").map(|p| std::fs::File::create(p).unwrap());
    let dump_at = std::env::var("QWC_TRACE_DUMP").ok().and_then(|v| v.parse::<u32>().ok());
    let ticks = 24 * 60 * 8;
    for _ in 0..ticks {
        if a.tick == 24 * 60 * 4 {
            a.start_race();
            b.start_race();
        }
        feed(&mut a, &mut ha);
        feed(&mut b, &mut hb);
        a.step();
        b.step();
        let t0 = std::time::Instant::now();
        let sum = a.checksum();
        timing += t0.elapsed();
        assert_eq!(sum, b.checksum(), "apart at tick {}", a.tick);
        digest = (digest ^ sum).wrapping_mul(0x100_0000_01b3);
        if let Some(t) = trace.as_mut() {
            use std::io::Write;
            let _ = writeln!(t, "{} {sum:016x}", a.tick);
            if Some(a.tick) == dump_at {
                let _ = writeln!(
                    t,
                    "{:#?}
{:#?}
{:?} {:?} {:?}",
                    a.players, a.teams, a.quaffle, a.holder, a.rng
                );
            }
        }
        if a.finished {
            break;
        }
    }
    eprintln!(
        "{} ticks, score {} - {}, caught by {:?}; checksum {:.0} us a tick; digest {digest:016x}",
        a.tick,
        a.teams[0].score,
        a.teams[1].score,
        a.race.as_ref().and_then(|r| r.caught),
        timing.as_secs_f64() * 1e6 / f64::from(a.tick)
    );
    assert!(a.teams.iter().any(|t| t.score > 0), "nothing happened");
    if let Ok(want) = std::env::var("QWC_EXPECT_DIGEST") {
        assert_eq!(format!("{digest:016x}"), want.trim(), "another platform played this match differently");
    }

    // one press more on one side shows on that tick
    let (mut a, mut b) = (real_match(&vfs), real_match(&vfs));
    let (mut ha, mut hb) = ([Hands(1), Hands(2)], [Hands(1), Hands(2)]);
    for _ in 0..24 * 20 {
        feed(&mut a, &mut ha);
        feed(&mut b, &mut hb);
        a.step();
        b.step();
    }
    let (i, pad) = ha[0].input(a.tick);
    let _ = hb[0].input(b.tick);
    a.set_input(0, i);
    a.set_seeker_pad(0, pad);
    b.set_input(0, Input { boost_pressed: !i.boost_pressed, boost_held: !i.boost_held, ..i });
    b.set_seeker_pad(0, pad);
    a.step();
    b.step();
    assert_ne!(a.checksum(), b.checksum(), "a different press left no trace");
}
