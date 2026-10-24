//! Run every shipped `.cli` script through the console. Runs only when `QWC_DATA_DIR` points at a repo root holding
//! `original/cd_files/` (e.g. `QWC_DATA_DIR=$PWD cargo test --release`); otherwise it is a no-op.

use std::path::PathBuf;

use qwc_assets::Vfs;
use qwc_sim::console::Error;
use qwc_sim::passing::PassAi;
use qwc_sim::Console;

#[test]
fn every_cli_line_reaches_the_registry() {
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let vfs = Vfs::open(root.join("original/cd_files")).unwrap();
    let files: Vec<String> = vfs.with_extension("cli").map(str::to_string).collect();
    assert_eq!(files.len(), 133); // the 132 archived files and the loose `root/screen.cli`
    let mut errors = Vec::new();
    for f in &files {
        let mut c = Console::new();
        errors.extend(c.run_script(&vfs.read(f).unwrap()).into_iter().map(|e| (f.clone(), e)));
    }
    for (f, e) in &errors {
        eprintln!("{f}: {e}");
    }
    assert_eq!(errors.len(), 1);
    assert!(matches!(&errors[0].1, Error::Unknown(k) if k == "GDR.Active"));

    // a team file sets its own slot only
    let path = files.iter().find(|f| f.to_lowercase().ends_with("england/team_3.cli")).expect("England's team_3");
    let mut c = Console::new();
    c.team = 1;
    c.run_script(&vfs.read(path).unwrap());
    let (zero, one) = (PassAi::from_console(&c, 0), PassAi::from_console(&c, 1));
    assert_eq!(zero, PassAi::from_console(&Console::new(), 0));
    eprintln!("England team_3 passing: {one:?}");
}

/// Five minutes of England against Bulgaria with the real team files and Formotion databases: positions stay finite
/// and near the pitch, the Formotions are picked, and goals are scored.
#[test]
fn a_real_match_runs_with_formotions() {
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let vfs = Vfs::open(root.join("original/cd_files")).unwrap();
    let mut c = Console::new();
    c.run_script(&vfs.read("root/CLI/autoexec.cli").unwrap());
    for (slot, team) in ["England", "Bulgaria"].iter().enumerate() {
        c.team = slot;
        c.run_script(&vfs.read(&format!("teams/{team}/team_3.cli")).unwrap());
    }
    let db = |p: &str| qwc_formats::formotion::parse(&vfs.read(p).unwrap()).unwrap();
    let (global, england, bulgaria) =
        (db("match/quidd/formotionDB.txt"), db("teams/England/formotionDB.txt"), db("teams/Bulgaria/formotionDB.txt"));
    let mut m = qwc_sim::Match::new(&c, qwc_sim::Rand::new(1));
    m.use_formotions(&global, [Some(&england), Some(&bulgaria)]);
    let mut picked = std::collections::BTreeSet::new();
    for _ in 0..24 * 300 {
        m.step();
        for p in &m.players {
            assert!(p.pos.iter().all(|v| v.is_finite()), "{p:?}");
            assert!(p.pos[0].abs() < 250.0 && p.pos[2].abs() < 150.0, "{p:?}");
        }
        for t in m.formation.as_ref().unwrap() {
            picked.extend(t.slots.iter().filter_map(|s| s.formotion.as_ref().map(|f| f.name.clone())));
        }
    }
    eprintln!("formotions used: {picked:?}");
    assert!(picked.len() > 10, "{picked:?}");
    assert!(m.teams[0].score + m.teams[1].score > 0);
}

/// Five minutes of England against Bulgaria with the Bludger move unlocked: the Bludgers and beaters fly their
/// formotions and stay near the pitch, the AI summons and launches its Bludger, and the Bludgers hit or are batted
/// away.
#[test]
fn the_bludgers_fly_in_a_real_match() {
    use qwc_sim::world::Event;
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let vfs = Vfs::open(root.join("original/cd_files")).unwrap();
    let mut c = Console::new();
    c.run_script(&vfs.read("root/CLI/autoexec.cli").unwrap());
    for (slot, team) in ["England", "Bulgaria"].iter().enumerate() {
        c.team = slot;
        c.run_script(&vfs.read(&format!("teams/{team}/team_3.cli")).unwrap());
    }
    let db = |p: &str| qwc_formats::formotion::parse(&vfs.read(p).unwrap()).unwrap();
    let (global, england, bulgaria) =
        (db("match/quidd/formotionDB.txt"), db("teams/England/formotionDB.txt"), db("teams/Bulgaria/formotionDB.txt"));
    let mut m = qwc_sim::Match::new(&c, qwc_sim::Rand::new(1));
    m.use_formotions(&global, [Some(&england), Some(&bulgaria)]);
    m.power.unlocked = [true; 4];
    m.power.uber = [true; 2];
    eprintln!("Bludger rules: {:?}, pots {:?}", m.bludger_rules, m.power.pots);
    let mut far = 0.0f64;
    // each flight's closest approach to the opposing active player
    let mut flight: [Option<(u32, f64)>; 2] = [None; 2];
    for _ in 0..24 * 300 {
        m.step();
        for (t, flight) in flight.iter_mut().enumerate() {
            let flying = m.teams[t].bludger.is_some_and(|mv| mv.flying);
            let target = m.teams[1 - t].active.map(|a| m.players[a].pos);
            match (flying, *flight, target) {
                (true, f, Some(p)) => {
                    let b = m.bludgers[t].pos;
                    let d = ((b[0] - p[0]).powi(2) + (b[1] - p[1]).powi(2) + (b[2] - p[2]).powi(2)).sqrt();
                    if std::env::var_os("QWC_BLUDGER_TRACE").is_some() {
                        let v = m.bludgers[t].vel;
                        let held = m.holder == m.teams[1 - t].active;
                        eprintln!(
                            "  {:.2} b ({:.1} {:.1} {:.1}) v ({:.0} {:.0} {:.0}) target ({:.1} {:.1} {:.1}) d {d:.1}{}",
                            m.tick as f64 / 24.0,
                            b[0],
                            b[1],
                            b[2],
                            v[0],
                            v[1],
                            v[2],
                            p[0],
                            p[1],
                            p[2],
                            if held { "" } else { " (not holding)" }
                        );
                    }
                    let (start, best) = f.unwrap_or((m.tick, f64::MAX));
                    *flight = Some((start, best.min(d)));
                }
                (false, Some((start, best)), _) => {
                    let how = m.events.iter().rev().take(40).find_map(|e| match *e {
                        Event::BludgerHit { team, .. } if team == t => Some("a hit"),
                        Event::BeaterBlock { player, .. } if m.players[player].team != t => Some("blocked"),
                        Event::KickOff { .. } => Some("a kick-off"),
                        _ => None,
                    });
                    eprintln!(
                        "team {t}'s flight from {:.1} s for {} ticks: closest {best:.1}, ended by {}",
                        start as f64 / 24.0,
                        m.tick - start,
                        how.unwrap_or("the AI's choice or the ball")
                    );
                    *flight = None;
                }
                _ => {}
            }
        }
        for b in &m.bludgers {
            assert!(b.pos.iter().all(|v| v.is_finite()), "{b:?}");
            let r = b.pos[0].abs().max(b.pos[2].abs());
            if r > far && r > 110.0 && std::env::var_os("QWC_BLUDGER_TRACE").is_some() {
                eprintln!("  far: {:.2} s {:?} {:?} {:?}", m.tick as f64 / 24.0, b.state, b.pos, b.vel);
            }
            far = far.max(r);
        }
        for t in &m.teams {
            for &p in &t.beaters {
                assert!(m.players[p].pos.iter().all(|v| v.is_finite()));
            }
        }
    }
    let count = |f: &dyn Fn(&Event) -> bool| m.events.iter().filter(|e| f(e)).count();
    let summons = count(&|e| matches!(e, Event::BludgerSummon { .. }));
    let launches = count(&|e| matches!(e, Event::BludgerLaunch { .. }));
    let hits = count(&|e| matches!(e, Event::BludgerHit { .. }));
    let blocks = count(&|e| matches!(e, Event::BeaterBlock { .. }));
    let fumbles = count(&|e| matches!(e, Event::Fumble { .. }));
    eprintln!(
        "summons {summons}, launches {launches}, hits {hits}, fumbles {fumbles}, blocks {blocks}; farthest Bludger {far:.0}; \
         score {} - {}",
        m.teams[0].score, m.teams[1].score
    );
    assert!(summons > 0 && launches > 0, "no Bludger move in five minutes");
    assert!(far < 250.0);
}

/// The Snitch flies two minutes round every stadium's spline: it keeps moving forward, stays near the trail, and
/// laps at about its cruising speed.
#[test]
fn the_snitch_laps_every_stadium() {
    use qwc_sim::seeker::{SeekerRules, Snitch};
    use qwc_sim::spline::Spline;
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let vfs = Vfs::open(root.join("original/cd_files")).unwrap();
    let r = SeekerRules::from_console(&Console::new());
    let files: Vec<String> = vfs.with_extension("qsp").map(str::to_string).collect();
    assert_eq!(files.len(), 15);
    for f in &files {
        let q = qwc_formats::qsp::Qsp::parse(&vfs.read(f).unwrap()).unwrap();
        let s = Spline::from_qsp(&q.splines[0], r.spline_base_scale as f64);
        let mut n = Snitch::start(&s, &r);
        let (start, mut worst) = (n.param, 0.0f64);
        for _ in 0..24 * 120 {
            n.update(&s, &r);
            let p = s.position(n.param);
            let d = ((n.pos[0] - p[0]).powi(2) + (n.pos[1] - p[1]).powi(2) + (n.pos[2] - p[2]).powi(2)).sqrt();
            worst = worst.max(d);
            assert!(n.pos.iter().all(|v| v.is_finite()));
        }
        let speed = (n.vel[0].powi(2) + n.vel[1].powi(2) + n.vel[2].powi(2)).sqrt();
        eprintln!(
            "{f}: {} points, start {start}, after 2 min {:.1} points ({:.2} laps), speed {speed:.1}, farthest from the line {worst:.2}",
            s.len(),
            n.param - start,
            (n.param - start) / s.len() as f64
        );
        assert!(worst < 6.0 && (n.param - start) / s.len() as f64 > 0.9 && (25.0..33.0).contains(&speed));
    }
}

/// A whole match on the England stadium: the power bars fill, the seekers race round the stadium's spline, and the
/// Snitch is caught, which ends the match.
#[test]
fn a_real_match_ends_with_the_snitch_caught() {
    use qwc_sim::world::{Event, Role};
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let vfs = Vfs::open(root.join("original/cd_files")).unwrap();
    let mut c = Console::new();
    c.run_script(&vfs.read("root/CLI/autoexec.cli").unwrap());
    for (slot, team) in ["England", "Bulgaria"].iter().enumerate() {
        c.team = slot;
        c.run_script(&vfs.read(&format!("teams/{team}/team_3.cli")).unwrap());
    }
    let db = |p: &str| qwc_formats::formotion::parse(&vfs.read(p).unwrap()).unwrap();
    let (global, england, bulgaria) =
        (db("match/quidd/formotionDB.txt"), db("teams/England/formotionDB.txt"), db("teams/Bulgaria/formotionDB.txt"));
    let mut m = qwc_sim::Match::new(&c, qwc_sim::Rand::new(1));
    m.use_formotions(&global, [Some(&england), Some(&bulgaria)]);
    let q = qwc_formats::qsp::Qsp::parse(&vfs.read("stadium/england/gfx/ENG0003.QSP").unwrap()).unwrap();
    m.use_spline(qwc_sim::spline::Spline::from_qsp(&q.splines[0], m.race_rules.seeker.spline_base_scale as f64));
    let mut race_start = None;
    // how far outside the walls' ellipse (as `x^2/a^2 + z^2/b^2`) and past the chasers' end lines anyone got
    let (mut worst, mut worst_x) = (0.0f64, 0.0f64);
    for _ in 0..24 * 60 * 20 {
        m.step();
        if race_start.is_none() && m.race.is_some() {
            race_start = Some(m.tick);
        }
        for p in m.players.iter().filter(|p| matches!(p.role, Role::Chaser | Role::Keeper)) {
            worst = worst.max((p.pos[0] / 112.5).powi(2) + (p.pos[2] / 42.5).powi(2));
            if p.role == Role::Chaser {
                worst_x = worst_x.max(p.pos[0].abs());
            }
        }
        if m.finished {
            break;
        }
    }
    let start = race_start.expect("the Seeker game never started");
    let end = m.events.iter().find_map(|e| match e {
        Event::Race { tick, event: qwc_sim::race::RaceEvent::Caught { team } } => Some((*tick, *team)),
        _ => None,
    });
    let (end, team) = end.expect("the Snitch was not caught in 20 minutes");
    eprintln!(
        "race from {:.0} s to {:.0} s, caught by team {team}; final {} - {}",
        start as f64 / 24.0,
        end as f64 / 24.0,
        m.teams[0].score,
        m.teams[1].score
    );
    assert!(m.finished && m.teams[team].score >= 150);
    // the walls (`qwc_sim::walls`): a mover is pushed back onto a wall it steps past
    eprintln!("farthest out: ellipse {worst:.3}, chasers' |x| {worst_x:.1}");
    assert!(worst < 1.1 && worst_x < 93.0);
    let r = m.race.as_ref().unwrap();
    assert!(r.seekers.iter().all(|s| s.pos.iter().all(|v| v.is_finite())));
}

/// The passing challenge with the player passing every two seconds: the description leaves team 0's three chasers, the rings follow the passes, and
/// within its two minutes the challenge is over one way or the other with every pass counted.
#[test]
fn the_passing_challenge_runs_to_its_end() {
    use qwc_sim::challenge::{Challenge, Event, Phase};
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let vfs = Vfs::open(root.join("original/cd_files")).unwrap();
    let mut c = Console::new();
    c.run_script(&vfs.read("root/CLI/autoexec.cli").unwrap());
    c.team = 0;
    c.run_script(&vfs.read("teams/Gryff/team_0.cli").unwrap());
    c.run_script(&vfs.read("match/cpass/Challenge.cli").unwrap());
    let desc = qwc_formats::matchdesc::MatchDesc::parse(&vfs.read("match/cpass/matchdesc.txt").unwrap()).unwrap();
    let db = qwc_formats::formotion::parse(&vfs.read("match/cpass/formotionDB.txt").unwrap()).unwrap();
    let mut m = qwc_sim::Match::new(&c, qwc_sim::Rand::new(4));
    m.use_formotions(&db, [None, None]);
    m.use_description(&desc);
    assert_eq!(m.challenge, Some(1));
    assert_eq!(m.players.iter().filter(|p| p.present).count(), 3);
    assert_eq!(m.holder, Some(m.teams[0].chasers[0]));
    // the player carries and passes every two seconds (Action held for 10 ticks), the stick at rest
    m.set_human(0, true);
    let mut ch = Challenge::new(1);
    ch.start(&mut m);
    let mut passes = 0;
    for tick in 0..24 * 200 {
        let k = tick % 48;
        let input = qwc_sim::human::Input { action_pressed: k == 0, action_held: k < 10, ..Default::default() };
        m.set_input(0, input);
        if ch.plays() {
            m.step();
        }
        for e in ch.tick(&mut m) {
            if let Event::Pass { outcome, .. } = e {
                passes += 1;
                if std::env::var_os("QWC_TEST_LOG").is_some() {
                    let last = m.events.iter().rev().find_map(|e| match e {
                        qwc_sim::world::Event::Pass { from, receiver, .. } => Some((*from, *receiver)),
                        _ => None,
                    });
                    eprintln!(
                        "pass {passes}: outcome {outcome}, green ring {}, rings {:?}, {last:?}",
                        ch.rings.green, ch.rings.pos
                    );
                }
            }
        }
        if ch.over() {
            break;
        }
        assert!(
            m.holder.is_none_or(|h| m.players[h].team == 0 && m.players[h].present),
            "{:?} {:?}",
            m.holder,
            m.events.last()
        );
        assert!(m.players.iter().filter(|p| p.present).all(|p| p.pos.iter().all(|v| v.is_finite())));
    }
    eprintln!("passing challenge: {:?} after {:.1} s, {passes} passes, {:?}", ch.phase, ch.time(), ch.rings.counts);
    assert!(ch.over());
    assert!(passes > 0);
    if ch.phase == Phase::Done {
        assert_eq!(ch.count, 10);
    }
}

/// The Beaters' challenge with a player who flies at the nearest target and presses Boost whenever the move is
/// allowed: the beaters summon the Bludger, it flies at the targets, and the attempts run the challenge to its end.
#[test]
fn the_beaters_challenge_runs_to_its_end() {
    use qwc_sim::challenge::{time_limit, Challenge, Event};
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let vfs = Vfs::open(root.join("original/cd_files")).unwrap();
    let mut c = Console::new();
    c.run_script(&vfs.read("root/CLI/autoexec.cli").unwrap());
    c.team = 0;
    c.run_script(&vfs.read("teams/Gryff/team_0.cli").unwrap());
    c.run_script(&vfs.read("match/cbnb/Challenge.cli").unwrap());
    let d = qwc_formats::matchdesc::MatchDesc::parse(&vfs.read("match/cbnb/matchdesc.txt").unwrap()).unwrap();
    let mut m = qwc_sim::Match::new(&c, qwc_sim::Rand::new(9));
    m.use_formotions(
        &qwc_formats::formotion::parse(&vfs.read("match/cbnb/formotionDB.txt").unwrap()).unwrap(),
        [None, None],
    );
    m.use_description(&d);
    assert_eq!(m.challenge, Some(4));
    assert!(m.bludgers[0].present && !m.bludgers[1].present);
    m.set_human(0, true);
    let mut ch = Challenge::new(4);
    ch.start(&mut m);
    let (mut hits, mut attempts) = (0, 0);
    for tick in 0..time_limit(4) + 200 {
        let player = m.teams[0].chasers[0];
        let p = m.players[player].pos;
        let goal = (0..5).filter(|&i| !ch.targets.hit[i]).map(|i| ch.targets.pos(i)).min_by(|a, b| {
            let d = |t: &[f64; 3]| (t[0] - p[0]).powi(2) + (t[2] - p[2]).powi(2);
            d(a).total_cmp(&d(b))
        });
        // the stick flies the chaser at the nearest target, and once the Bludger flies, the Bludger at its aim
        let flying = m.teams[0].bludger.is_some_and(|mv| mv.flying);
        let (from, to) = match (flying, m.challenge_aim) {
            (true, Some(aim)) => (m.bludgers[0].pos, Some(aim)),
            _ => (p, goal),
        };
        let stick = to.map_or([0.0; 3], |g| {
            let (dx, dz) = (g[0] - from[0], g[2] - from[2]);
            let l = (dx * dx + dz * dz).sqrt().max(1e-9);
            [dx / l, 0.0, dz / l]
        });
        let boost = m.bludgers_allowed && tick % 12 == 0;
        m.set_input(0, qwc_sim::human::Input { stick, boost_pressed: boost, boost_held: boost, ..Default::default() });
        if ch.plays() {
            m.step();
        }
        for e in ch.tick(&mut m) {
            match e {
                Event::TargetHit(_) => hits += 1,
                Event::Pass { .. } => attempts += 1,
                _ => {}
            }
        }
        if ch.over() {
            break;
        }
        if std::env::var_os("QWC_BLUDGER_TRACE").is_some() && tick % 12 == 0 {
            let b = m.bludgers[0].pos;
            let aim = m.challenge_aim.unwrap_or([0.0; 3]);
            let d = ((b[0] - aim[0]).powi(2) + (b[1] - aim[1]).powi(2) + (b[2] - aim[2]).powi(2)).sqrt();
            eprintln!(
                "  {:.1} s {:?} b ({:.1} {:.1} {:.1}) v {:?} aim ({:.1} {:.1} {:.1}) d {d:.1} player ({:.1} {:.1} {:.1}) range {:?} move {:?}",
                tick as f64 / 24.0,
                m.bludgers[0].state,
                b[0],
                b[1],
                b[2],
                m.bludgers[0].vel.map(|v| v.round()),
                aim[0],
                aim[1],
                aim[2],
                p[0],
                p[1],
                p[2],
                ch.targets.in_range,
                m.teams[0].bludger.map(|mv| (mv.flying, mv.tick))
            );
        }
        assert!(m.bludgers[0].pos.iter().all(|v| v.is_finite()));
    }
    let summons = m.events.iter().filter(|e| matches!(e, qwc_sim::world::Event::BludgerSummon { .. })).count();
    eprintln!(
        "Beaters' challenge: {:?} after {:.1} s, {attempts} attempts, {hits} targets hit, {summons} summons, {:?}",
        ch.phase,
        ch.time(),
        ch.rings.counts
    );
    assert!(ch.over());
    assert!(summons > 0, "the Bludger was never summoned");
}

/// Challenges 2, 3, 5 and 6 on the AI: each sets up its players (and 3 its race) and ends within its limit.
#[test]
fn the_other_challenges_run_to_their_end() {
    use qwc_sim::challenge::{time_limit, Challenge};
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let vfs = Vfs::open(root.join("original/cd_files")).unwrap();
    for (kind, desc, stadium) in
        [(2, "ctns", "chal_01"), (3, "cseek", "chal_02"), (5, "csmt", "chal_01"), (6, "ccombo", "chal_01")]
    {
        let mut c = Console::new();
        c.run_script(&vfs.read("root/CLI/autoexec.cli").unwrap());
        c.team = 0;
        c.run_script(&vfs.read("teams/Gryff/team_0.cli").unwrap());
        let cli = format!("match/{desc}/Challenge.cli");
        if vfs.exists(&cli) {
            c.run_script(&vfs.read(&cli).unwrap());
        }
        let d = qwc_formats::matchdesc::MatchDesc::parse(&vfs.read(&format!("match/{desc}/matchdesc.txt")).unwrap())
            .unwrap();
        let mut m = qwc_sim::Match::new(&c, qwc_sim::Rand::new(9));
        if let Ok(f) = vfs.read(&format!("match/{desc}/formotionDB.txt")) {
            m.use_formotions(&qwc_formats::formotion::parse(&f).unwrap(), [None, None]);
        }
        let qsp = vfs
            .with_extension("qsp")
            .find(|f| f.to_lowercase().starts_with(&format!("stadium/{stadium}/")))
            .map(str::to_string);
        if let Some(q) = qsp {
            let q = qwc_formats::qsp::Qsp::parse(&vfs.read(&q).unwrap()).unwrap();
            m.use_spline(qwc_sim::spline::Spline::from_qsp(
                &q.splines[0],
                m.race_rules.seeker.spline_base_scale as f64,
            ));
        }
        m.use_description(&d);
        let mut ch = Challenge::new(kind);
        ch.start(&mut m);
        for _ in 0..time_limit(kind) + 200 {
            if ch.plays() {
                m.step();
            }
            ch.tick(&mut m);
            if ch.over() {
                break;
            }
        }
        eprintln!(
            "challenge {kind} ({desc}): {:?} after {:.1} s, count {}, {:?}",
            ch.phase,
            ch.time(),
            ch.count,
            ch.rings.counts
        );
        assert!(ch.over(), "challenge {kind}");
        assert!(m.players.iter().filter(|p| p.present).all(|p| p.pos.iter().all(|v| v.is_finite())));
    }
}

/// Two minutes of England against Bulgaria: the pass builder flies each receiver to the target (`0x425c10`), so a
/// pass that is not intercepted arrives within reach of him instead of snapping onto him from afar.
#[test]
fn passes_arrive_at_their_receivers() {
    use qwc_sim::world::{Event, FlightKind};
    let Some(root) = std::env::var_os("QWC_DATA_DIR").map(PathBuf::from) else {
        eprintln!("QWC_DATA_DIR not set; skipping");
        return;
    };
    let vfs = Vfs::open(root.join("original/cd_files")).unwrap();
    let mut c = Console::new();
    c.run_script(&vfs.read("root/CLI/autoexec.cli").unwrap());
    for (slot, team) in ["England", "Bulgaria"].iter().enumerate() {
        c.team = slot;
        c.run_script(&vfs.read(&format!("teams/{team}/team_3.cli")).unwrap());
    }
    c.run_script(&vfs.read("stadium/england/game/match.cli").unwrap());
    let db = |p: &str| qwc_formats::formotion::parse(&vfs.read(p).unwrap()).unwrap();
    let (global, england, bulgaria) =
        (db("match/quidd/formotionDB.txt"), db("teams/England/formotionDB.txt"), db("teams/Bulgaria/formotionDB.txt"));
    let mut m = qwc_sim::Match::new(&c, qwc_sim::Rand::new(1));
    m.use_formotions(&global, [Some(&england), Some(&bulgaria)]);
    let (mut pending, mut seen, mut arrivals, mut worst) = (Vec::new(), 0, 0, 0.0f64);
    for _ in 0..24 * 120 {
        m.step();
        for e in &m.events[seen..] {
            if let Event::Pass { receiver, arrival, .. } = *e {
                pending.push((arrival, receiver));
            }
        }
        seen = m.events.len();
        let (tick, quaffle) = (m.tick, m.quaffle);
        pending.retain(|&(arrival, r)| {
            if tick + 1 != arrival {
                return tick + 1 < arrival;
            }
            // an interception ended it before its arrival
            if !matches!(m.flight, Some(f) if f.kind == FlightKind::Pass { receiver: r }) {
                return false;
            }
            let p = m.players[r].pos;
            let d = ((p[0] - quaffle[0]).powi(2) + (p[1] - quaffle[1]).powi(2) + (p[2] - quaffle[2]).powi(2)).sqrt();
            arrivals += 1;
            worst = worst.max(d);
            false
        });
    }
    eprintln!("{arrivals} passes reached their arrival; the farthest receiver was {worst:.2} from the ball");
    assert!(arrivals > 10 && worst < 2.0);
}
