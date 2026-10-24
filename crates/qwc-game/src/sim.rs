//! The `sim` command: run the gameplay simulation headless and print what happens.

use qwc_assets::Vfs;
use qwc_formats::formotion;
use qwc_sim::race::RaceEvent;
use qwc_sim::spline::Spline;
use qwc_sim::world::Event;
use qwc_sim::{Console, Match, Rand};

use crate::{fail, Args};

/// A match set up as the game sets one up: `root/CLI/autoexec.cli`, then each team file into its slot, then the
/// stadium's `game/match.cli` (the pitch: goal height, height-map scale and choice) and `snitch.cli` if it has one
/// (`stadium/<stadium>/gfx`), its height maps, its first spline for the Seeker game and the Formotions (the normal
/// match's database and each team's own, next to its `.cli` file); `rand()` from `seed`. **[I]** a stadium without
/// `heightmap.csv` keeps the table as it was (`RE_NOTES.md`), which after the front end is `sum_fe`'s.
///
/// `desc` is the match description's folder (`match/<desc>`): `quidd` for a match; for a challenge its
/// `Challenge.cli` runs after the stadium's settings, its `formotionDB.txt` is the global database and its
/// `matchdesc.txt` leaves only the challenge's players (`0x4596f0`, **[I]** the order).
///
/// Not the original's: the Gameplay packs on (`qwc_packs`) have the last word. A team's `tuning/teams/<folder>/*.cli`
/// run into its slot after its own file, and `tuning/*.cli` after everything else, the lowest pack first.
pub(crate) fn new_match(vfs: &Vfs, teams: [&str; 2], stadium: &str, desc: &str, seed: u32) -> (Match, Console) {
    let mut console = Console::new();
    // a missing script leaves the console's defaults (the challenges' opponents have no team files)
    let run = |c: &mut Console, path: &str| match vfs.read(path) {
        Ok(bytes) => {
            for e in c.run_script(&bytes) {
                eprintln!("{path}: {e}");
            }
        }
        Err(e) => eprintln!("{path}: {e}"),
    };
    run(&mut console, "root/CLI/autoexec.cli");
    let packs = qwc_packs::current();
    let tune = |c: &mut Console, path: &std::path::Path| match std::fs::read(path) {
        Ok(bytes) => {
            for e in c.run_script(&bytes) {
                eprintln!("{}: {e}", path.display());
            }
        }
        Err(e) => eprintln!("{}: {e}", path.display()),
    };
    for (slot, path) in teams.into_iter().enumerate() {
        console.team = slot;
        run(&mut console, path);
        // `teams/<folder>/team_N.cli`
        let folder = path.split(['/', '\\']).nth(1).unwrap_or("");
        for t in packs.team_tuning(folder) {
            tune(&mut console, &t);
        }
    }
    // the stadium: its match settings, then (`0x44acf0`: SplineBaseScale back to 0.015) its own Snitch script
    let gfx = format!("stadium/{stadium}/gfx/");
    for f in [format!("stadium/{stadium}/game/match.cli"), format!("{gfx}snitch.cli")] {
        if vfs.exists(&f) {
            run(&mut console, &f);
        }
    }
    let mut qsps: Vec<String> = vfs
        .with_extension("qsp")
        .filter(|f| f.to_lowercase().starts_with(&gfx.to_lowercase()))
        .map(str::to_string)
        .collect();
    qsps.sort();
    let challenge_cli = format!("match/{desc}/Challenge.cli");
    if vfs.exists(&challenge_cli) {
        run(&mut console, &challenge_cli);
    }
    for t in packs.tuning_scripts() {
        tune(&mut console, &t);
    }
    let rules: Vec<String> = packs.gameplay().map(|p| format!("{} {}", p.manifest.id, p.manifest.version)).collect();
    if !rules.is_empty() {
        eprintln!("gameplay packs: {}", rules.join(", "));
    }
    let mut m = Match::new(&console, Rand::new(seed));
    let maps = [format!("{gfx}heightmap.csv"), "stadium/sum_fe/gfx/heightmap.csv".into()]
        .into_iter()
        .find_map(|f| qwc_formats::heightmap::parse(&vfs.read(&f).ok()?).ok());
    if let Some(maps) = maps {
        let map = |m: &Vec<Vec<f64>>| std::array::from_fn(|r| std::array::from_fn(|c| m[r][c]));
        m.use_height_maps(maps.iter().map(map).collect());
    }
    match qsps.first() {
        Some(f) => {
            let q = qwc_formats::qsp::Qsp::parse(&vfs.read(f).unwrap_or_else(|e| fail(e))).unwrap_or_else(|e| fail(e));
            m.use_spline(Spline::from_qsp(&q.splines[0], m.race_rules.seeker.spline_base_scale as f64));
        }
        None => eprintln!("no .qsp under {gfx}: the Seeker game has no race"),
    }
    let db = |path: &str| vfs.read(path).ok().and_then(|d| formotion::parse(&d).ok());
    let team_db = |cli: &str| db(&format!("{}/formotionDB.txt", cli.rsplit_once('/').map_or("", |(d, _)| d)));
    let global_db = format!("match/{desc}/formotionDB.txt");
    let quidd = desc.eq_ignore_ascii_case("quidd");
    match db(&global_db).or_else(|| db("match/quidd/formotionDB.txt")) {
        Some(global) if quidd => {
            let dbs = teams.map(team_db);
            m.use_formotions(&global, [dbs[0].as_ref(), dbs[1].as_ref()]);
        }
        Some(global) => m.use_formotions(&global, [None, None]),
        None => eprintln!("{global_db} not found: off-ball chasers use the placeholder movement"),
    }
    if let Ok(d) = vfs.read(&format!("match/{desc}/matchdesc.txt")) {
        match qwc_formats::matchdesc::MatchDesc::parse(&d) {
            Ok(d) => m.use_description(&d),
            Err(e) => eprintln!("match/{desc}/matchdesc.txt: {e}"),
        }
    }
    // the remaster's momentum for the formotions (the settings' `gameplay.momentum`; `QWC_MOMENTUM=ACCEL` sets it)
    m.momentum = match std::env::var("QWC_MOMENTUM").ok().and_then(|v| v.parse().ok()) {
        Some(a) => a,
        None if qwc_settings::Settings::load().gameplay.momentum => qwc_sim::formation::MOMENTUM,
        None => 0.0,
    };
    // the match's start: the toss and the race for the kick-off (a challenge keeps its own start)
    m.start_toss();
    (m, console)
}

/// `challenge [DESC [HOUSE [OPPONENT]]] [--stadium NAME] [--time SECS]`: a House Challenge as the live front end sets
/// one up (`crate::fematch`; DESC `cpass`, HOUSE `Gryff` and the OPPONENT team's folder `Slyth` by default, in
/// `chal_01`), its team an idle human's (the AI's with `--watch`), printing each second the carrier, how far its mates are, the rings (the passing challenge's) and the match camera's view;
/// with `--out FILE.png`, the world as the game draws it at the end (`--size`), the rings in it.
pub fn challenge(args: &Args, vfs: &'static Vfs) {
    let desc = args.models.first().map_or("cpass", String::as_str);
    let house = args.models.get(1).map_or("Gryff", String::as_str);
    let opponent = args.models.get(2).map_or("Slyth", String::as_str);
    let spec = crate::arena::MatchSpec {
        stadium: args.stadium.clone().unwrap_or_else(|| "chal_01".into()),
        // the challenges' hoop packages (their load sets'): the Beaters' targets', the others' rings
        hoops: if desc == "cbnb" { "hooptarg" } else { "hoopchal" }.into(),
        teams: [house.to_string(), opponent.to_string()],
        skill: 1,
        music: "chal".into(),
        sounds: None,
        desc: desc.to_string(),
    };
    let mut m = spec.start(vfs, 1);
    let Some(kind) = m.challenge else { crate::fail(format!("match/{desc} is not a challenge")) };
    // the player's team as the game has it, a human (idle here), or with `--watch` the AI's
    if !args.watch {
        m.set_human(0, true);
    }
    let mut ch = qwc_sim::challenge::Challenge::new(kind);
    ch.start(&mut m);
    let mut cam = crate::matchcam::MatchCamera::default();
    cam.cut();
    // ticks with a mate out of the picture, ticks, and the worst seen
    let mut framing = (0u32, 0u32, 0.0f32);
    // the view's turn each tick, in degrees
    let (mut last_dir, mut turns) = (None, Vec::new());
    let secs = if args.time > 0.0 { args.time } else { 30.0 };
    let f = |v: [f64; 3]| format!("({:.1}, {:.1}, {:.1})", v[0], v[1], v[2]);
    let g = |v: qwc_render::glam::Vec3| format!("({:.1}, {:.1}, {:.1})", v.x, v.y, v.z);
    eprintln!("hoops: {}", m.hoops.iter().map(|h| f(h.pos)).collect::<Vec<_>>().join(" "));
    // the world on the GPU, when a picture is asked for
    let mut world = args.out.as_ref().map(|_| {
        let gpu = qwc_render::Gpu::headless().unwrap_or_else(|e| crate::fail(e));
        let mut renderer = qwc_render::Renderer::new(&gpu.device, &gpu.queue, qwc_render::overlay::OVERLAY_FORMAT);
        renderer.shaders = crate::packs::shaders();
        let arena = crate::arena::Arena::load(&gpu, &mut renderer, vfs, &spec, &m);
        (gpu, renderer, arena)
    });
    // `QWC_CHAL_PASS=SECONDS,...`: the human passes then, the stick towards the green ring's mate (then the
    // camera every 3 ticks for 2 seconds)
    let passes: Vec<u32> = std::env::var("QWC_CHAL_PASS")
        .map(|v| v.split(',').filter_map(|s| s.trim().parse::<f64>().ok()).map(|s| (s * 24.0) as u32).collect())
        .unwrap_or_default();
    let mut watch_until = 0;
    let mut last_slots: Option<String> = None;
    let (mut cam_eye, mut cam_dir) = (qwc_render::glam::Vec3::ZERO, qwc_render::glam::Vec3::Z);
    let mut last_view: Option<crate::matchcam::View> = None;
    for tick in 0..(secs * 24.0) as u32 {
        if ch.plays() {
            // the stick held towards the ring for half a second from the press, the button for a few ticks
            let pressed = passes.iter().copied().filter(|&p| p <= tick && tick < p + 12).max();
            if let (Some(at), Some(h)) = (pressed, m.holder.or(m.teams[0].active)) {
                let ring = ch.rings.pos[ch.rings.green];
                let p = m.players[h].pos;
                let (dx, dz) = (ring[0] - p[0], ring[2] - p[2]);
                let l = (dx * dx + dz * dz).sqrt().max(1e-6);
                let input = qwc_sim::human::Input {
                    stick: [dx / l, 0.0, dz / l],
                    action_pressed: tick == at,
                    action_held: tick < at + 4,
                    ..Default::default()
                };
                m.set_input(0, input);
                if tick == at {
                    watch_until = tick + 48;
                    eprintln!("  [{:.2}] pass towards the green ring at {}", tick as f64 / 24.0, f(ring));
                }
            } else {
                // `QWC_CHAL_STICK=START,END,X,Z`: the move stick held (in the world, seconds) between those times
                let stick = std::env::var("QWC_CHAL_STICK").ok().and_then(|v| {
                    let n: Vec<f64> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    let t = tick as f64 / 24.0;
                    (n.len() == 4 && n[0] <= t && t < n[1]).then(|| [n[2], 0.0, n[3]])
                });
                m.set_input(0, qwc_sim::human::Input { stick: stick.unwrap_or_default(), ..Default::default() });
                if stick.is_some() && tick % 6 == 0 {
                    let h = m.holder.map(|h| m.players[h].pos);
                    eprintln!(
                        "  [{:.2}] stick: holder {:?} camera eye {} looking {}",
                        tick as f64 / 24.0,
                        h.map(f),
                        g(cam_eye),
                        g(cam_dir)
                    );
                }
            }
            m.step();
            if let Some((_, _, arena)) = &mut world {
                arena.ticked(&m, m.tick as f64 / 24.0);
            }
        }
        let events = ch.tick(&mut m);
        // `QWC_CHAL_SLOTS`: team 0's formation state and each slot's formotion whenever they change
        if std::env::var_os("QWC_CHAL_SLOTS").is_some() {
            if let Some(fm) = &m.formation {
                let t = &fm[0];
                let now: Vec<String> = t
                    .slots
                    .iter()
                    .map(|s| {
                        let name = s.formotion.as_ref().map_or("-".to_string(), |f| f.name.clone());
                        format!("{}:{name}{}", s.player, if s.done { "(done)" } else { "" })
                    })
                    .collect();
                let line = format!("state {} leader {:?} {}", t.current, t.leader, now.join(" "));
                if last_slots.as_deref() != Some(line.as_str()) {
                    eprintln!("  slots {tick:4} {line}");
                    last_slots = Some(line);
                }
            }
        }
        // `QWC_CHAL_TRACE=PLAYER`: that player's position, velocity and facing every tick
        if let Some(p) = std::env::var("QWC_CHAL_TRACE").ok().and_then(|v| v.parse::<usize>().ok()) {
            let pl = &m.players[p];
            eprintln!(
                "  trace {:4} pos {} vel {} fwd {} cmd {}",
                tick,
                f(pl.pos),
                f(pl.vel),
                f(pl.fwd),
                pl.cmd.is_some()
            );
        }
        // the remaster's framing of the mates (`QWC_CAM_NOKEEP` leaves it out, as the original's camera)
        let mut inputs = crate::matchcam::Inputs::of(&m, 0);
        if std::env::var_os("QWC_CAM_NOKEEP").is_some() {
            inputs.keep = [None; 2];
        }
        let view = cam.step(&inputs);
        (cam_eye, cam_dir) = (view.eye, view.dir);
        last_view = Some(view);
        // how far into the picture the mates are (a fraction of the half width or height; past 1 or behind is out)
        let mates = crate::matchcam::Inputs::of(&m, 0).keep;
        let worst = mates.iter().flatten().map(|&p| framed(&view, p)).fold(0.0f32, f32::max);
        framing.0 += (worst > 1.0) as u32;
        framing.1 += 1;
        framing.2 = framing.2.max(worst);
        if let Some(d) = last_dir {
            let turn = view.dir.angle_between(d).to_degrees();
            turns.push(turn);
            if turn > 3.5 && std::env::var_os("QWC_CAM_FRAME_LOG").is_some() {
                eprintln!("  [{:.2}] the view turned {turn:.1} degrees", tick as f64 / 24.0);
            }
        }
        last_dir = Some(view.dir);
        if std::env::var_os("QWC_CAM_FRAME_LOG").is_some() {
            let each: Vec<String> = mates.iter().flatten().map(|&p| format!("{:.2}", framed(&view, p))).collect();
            eprintln!(
                "  [{:.2}] mates at {} of the half frame; holder {:?}",
                tick as f64 / 24.0,
                each.join(" "),
                m.holder
            );
        }
        for e in events.iter().filter(|e| !matches!(e, qwc_sim::challenge::Event::Countdown(_))) {
            eprintln!("  [{:.2}] {e:?}", tick as f64 / 24.0);
        }
        if tick < watch_until && tick % 3 == 0 {
            eprintln!(
                "  [{:.2}] holder {:?} quaffle {} camera eye {} looking {}",
                tick as f64 / 24.0,
                m.holder,
                f(m.quaffle),
                g(view.eye),
                g(view.dir)
            );
        }
        if tick % 24 != 0 {
            continue;
        }
        let holder = m.holder;
        let mates: Vec<String> = m
            .players
            .iter()
            .enumerate()
            .filter(|(i, p)| p.team == 0 && p.present && Some(*i) != holder)
            .map(|(_, p)| {
                let d = holder
                    .map_or(0.0, |h| (0..3).map(|k| (p.pos[k] - m.players[h].pos[k]).powi(2)).sum::<f64>().sqrt());
                format!("{:?} {} at {d:.1}", p.role, f(p.pos))
            })
            .collect();
        let opponents: Vec<String> = m
            .players
            .iter()
            .filter(|p| p.team == 1 && p.present)
            .map(|p| format!("{:?} {}", p.role, f(p.pos)))
            .collect();
        eprintln!(
            "{:5.1} s {:?}: holder {} (team {}) quaffle {}; mates {}; opponents {}; rings {} {} green {}; camera eye {}              looking {}",
            tick as f64 / 24.0,
            ch.phase,
            holder.map_or("none".into(), |h| f(m.players[h].pos)),
            holder.map_or("-".into(), |h| m.players[h].team.to_string()),
            f(m.quaffle),
            mates.join(", "),
            opponents.join(", "),
            f(ch.rings.pos[0]),
            f(ch.rings.pos[1]),
            ch.rings.green,
            g(view.eye),
            g(view.dir),
        );
        if ch.over() {
            break;
        }
    }
    eprintln!("{:?}: {} passes, {:.1} s", ch.phase, ch.count, ch.time());
    eprintln!(
        "mates out of the picture {} of {} ticks (worst {:.2} of the half frame)",
        framing.0, framing.1, framing.2
    );
    turns.sort_by(f32::total_cmp);
    let pct = |q: f32| turns.get(((turns.len() as f32 - 1.0) * q) as usize).copied().unwrap_or(0.0);
    eprintln!(
        "the view turned {:.2} degrees a tick at the median, {:.2} at 99%, {:.2} at most",
        pct(0.5),
        pct(0.99),
        pct(1.0)
    );
    if let (Some((gpu, mut renderer, mut arena)), Some(out)) = (world, &args.out) {
        arena.rings = (0..2)
            .filter(|_| kind == 1)
            .map(|i| {
                let p = ch.rings.pos[i];
                let v = qwc_render::glam::Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32);
                (v, ch.rings.size[i] as f32, ch.rings.colour[i].map(|x| x as f32))
            })
            .collect();
        let camera = arena.frame(&gpu, &renderer, &m, 1.0, m.tick as f64 / 24.0);
        // through the camera the run stepped (the arena's own has only seen this frame)
        let camera = last_view.map_or(camera, |v| v.camera());
        let target = qwc_render::overlay::OverlayTarget::new(&gpu.device, args.size);
        let depth = qwc_render::depth_view(&gpu.device, args.size);
        arena.draw_world(&gpu, &mut renderer, &target.view, &depth, args.size, &m, &camera, 1.0);
        crate::fe::write_png(out, args.size, target.read_rgba(&gpu).unwrap_or_else(|e| crate::fail(e)));
        eprintln!("wrote {}", out.display());
    }
}

/// `sim TEAM0.cli TEAM1.cli`: the match of [`new_match`] (`--stadium`, default `england`; `rand()` seed 1) for
/// `--time` seconds (default 60), or until the Snitch is caught.
pub fn sim(args: &Args, vfs: &Vfs) {
    let [t0, t1] = &args.models[..] else { crate::usage() };
    let stadium = args.stadium.as_deref().unwrap_or("england");
    let (mut m, _) = new_match(vfs, [t0, t1], stadium, "quidd", 1);
    let secs = if args.time > 0.0 { args.time } else { 60.0 };
    // `--out FILE.json` also records a replay at 12 frames per second
    let mut frames: Vec<Vec<f64>> = Vec::new();
    // `QWC_SIM_SEEKERS`: the seekers' places and speeds each second
    let seekers = std::env::var_os("QWC_SIM_SEEKERS").is_some();
    for tick in 0..(secs * 24.0) as u32 {
        m.step();
        if seekers && tick % 24 == 0 {
            let s = [m.teams[0].seeker, m.teams[1].seeker].map(|i| &m.players[i]);
            let speed = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            println!(
                "{:5.1} s  seekers ({:7.1} {:6.1} {:7.1}) {:5.1}/s  ({:7.1} {:6.1} {:7.1}) {:5.1}/s",
                tick as f64 / 24.0,
                s[0].pos[0],
                s[0].pos[1],
                s[0].pos[2],
                speed(s[0].vel),
                s[1].pos[0],
                s[1].pos[1],
                s[1].pos[2],
                speed(s[1].vel)
            );
        }
        if args.out.is_some() && tick % 2 == 0 {
            let mut f: Vec<f64> = m.players.iter().flat_map(|p| [p.pos[0], p.pos[2]]).collect();
            f.extend([m.quaffle[0], m.quaffle[2], m.quaffle[1]]);
            f.push(if m.holder.is_some() { 1.0 } else { 0.0 });
            // the Snitch (x, z, y) once the race is on
            match &m.race {
                Some(r) => f.extend([1.0, r.snitch.pos[0], r.snitch.pos[2], r.snitch.pos[1]]),
                None => f.extend([0.0; 4]),
            }
            frames.push(f);
        }
        if m.finished {
            break;
        }
    }
    let name = |p: usize| format!("team {} {:?}", m.players[p].team, m.players[p].role);
    let at = |tick: u32| tick as f64 / 24.0;
    let mut score = [0; 2];
    let mut log: Vec<(u32, &str, String)> = Vec::new();
    for e in &m.events {
        let (tick, kind, text) = match *e {
            Event::WindUp { tick, player } => (tick, "windup", format!("{} winds up", name(player))),
            Event::Pass { tick, from, receiver, arrival, in_cone, volley } => (
                tick,
                "pass",
                format!(
                    "{} passes to {} ({} ticks, {}{})",
                    name(from),
                    name(receiver),
                    arrival - tick,
                    if in_cone { "in the cone" } else { "catch race" },
                    if volley { ", volley" } else { "" }
                ),
            ),
            Event::Catch { tick, player } => (tick, "catch", format!("{} catches", name(player))),
            Event::ShotWindUp { tick, player, ticks } => {
                (tick, "shotwindup", format!("{} winds up a shot ({ticks} ticks)", name(player)))
            }
            Event::Shot { tick, player, hoop, power, goal } => (
                tick,
                "shot",
                format!("{} shoots at hoop {hoop} (power {power:.2}, due at {:.2} s)", name(player), at(goal)),
            ),
            Event::Touch { tick, player, state } => {
                let what = match state {
                    9 => "saves the shot",
                    6 => "intercepts the pass",
                    _ => "picks up the ball",
                };
                (tick, if state == 9 { "save" } else { "intercept" }, format!("{} {what}", name(player)))
            }
            Event::Goal { tick, team, hoop, .. } => {
                score[team] += 10;
                (tick, "goal", format!("GOAL for team {team} at hoop {hoop}: {} - {}", score[0], score[1]))
            }
            Event::SpecialMove { tick, player, hoop, .. } => {
                (tick, "special", format!("{} makes the special move at hoop {hoop}", name(player)))
            }
            Event::UberMove { tick, player, hoop, .. } => {
                (tick, "special", format!("{} makes the team's uber move at hoop {hoop}", name(player)))
            }
            Event::SpecialTackle { tick, tackler, victim, .. } => (
                tick,
                "special",
                format!("{} takes the ball from {} with the special tackle", name(tackler), name(victim)),
            ),
            Event::GoalCutscene { tick, kind, .. } => {
                let what = if kind == 3 { "the celebration" } else { "the crowd's" };
                (tick, "goal", format!("the goal's cutscene: {what}"))
            }
            Event::KickOff { tick, team } => (tick, "kickoff", format!("team {team} kicks off")),
            Event::KickOffWindow { tick } => (tick, "toss", "the kick-off's window opens".to_string()),
            Event::Toss { tick, team, player } => {
                (tick, "toss", format!("team {team} wins the toss: {} flies to take it", name(player)))
            }
            Event::Lunge { tick, player } => (tick, "lunge", format!("{} lunges", name(player))),
            Event::LungeMiss { tick, player } => (tick, "lunge-miss", format!("{}'s lunge misses", name(player))),
            Event::Tackle { tick, tackler, victim, end, lunge } => (
                tick,
                "tackle",
                format!(
                    "{} tackles {}{} (ball changes hands at {:.2} s)",
                    name(tackler),
                    name(victim),
                    if lunge { " at the end of his lunge" } else { "" },
                    at(end)
                ),
            ),
            Event::SeekerGame { tick } => {
                (tick, "seeker", "the power bars are full: the Seeker game starts".to_string())
            }
            Event::Whiff { tick, player } => (tick, "whiff", format!("{} misses a dodging carrier", name(player))),
            Event::Dodge { tick, player, kind } => (tick, "dodge", format!("{} dodges ({kind})", name(player))),
            Event::Race { tick, event } => match event {
                RaceEvent::Grab { team, allowed } => (
                    tick,
                    "grab",
                    format!("team {team} Seeker reaches for the Snitch{}", if allowed { "" } else { " (too early)" }),
                ),
                RaceEvent::Missed { team } => (tick, "missed", format!("team {team} Seeker misses the Snitch")),
                RaceEvent::Caught { team } => {
                    score[team] += 150;
                    (tick, "caught", format!("team {team} Seeker CATCHES THE SNITCH: {} - {}", score[0], score[1]))
                }
                RaceEvent::Attack { team } => (tick, "attack", format!("team {team} Seeker goes for the other")),
                RaceEvent::Hit { team } => (tick, "hit", format!("team {team} Seeker is knocked back")),
                RaceEvent::Bump => (tick, "bump", "the seekers bump".to_string()),
                RaceEvent::Stance { team, on } => (
                    tick,
                    "stance",
                    format!(
                        "team {team} Seeker {} the Snitch",
                        if on { "is within reach of" } else { "falls out of reach of" }
                    ),
                ),
            },
            Event::BludgerSummon { tick, team, beater } => {
                (tick, "summon", format!("team {team}: {} summons the Bludger", name(beater)))
            }
            Event::BeaterSwing { tick, player } => (tick, "swing", format!("{} swings", name(player))),
            Event::BludgerLaunch { tick, team } => (tick, "launch", format!("team {team}'s Bludger is launched")),
            Event::BludgerHit { tick, team, victim, front } => (
                tick,
                "bludgerhit",
                format!("team {team}'s Bludger hits {}{}", name(victim), if front { " from the front" } else { "" }),
            ),
            Event::Fumble { tick, player } => (tick, "fumble", format!("{} drops the Quaffle", name(player))),
            Event::BeaterBlock { tick, player } => (tick, "block", format!("{} bats the Bludger away", name(player))),
        };
        println!("{:7.2} s  {text}", at(tick));
        log.push((tick, kind, text));
    }
    println!("final score {} - {}", m.teams[0].score, m.teams[1].score);
    if let Some(out) = &args.out {
        let team_name = |cli: &str| {
            let parts: Vec<&str> = cli.split('/').collect();
            let n = parts.len();
            if n >= 2 {
                format!("{} ({})", parts[n - 2], parts[n - 1].trim_end_matches(".cli"))
            } else {
                cli.to_string()
            }
        };
        let num = |v: f64| format!("{:.2}", v).trim_end_matches('0').trim_end_matches('.').to_string();
        let mut j = String::new();
        j.push_str(&format!("{{\"hz\":12,\"teams\":[\"{}\",\"{}\"],", team_name(t0), team_name(t1)));
        j.push_str("\"players\":[");
        let ps: Vec<String> =
            m.players.iter().map(|p| format!("{{\"team\":{},\"role\":\"{:?}\"}}", p.team, p.role)).collect();
        j.push_str(&ps.join(","));
        j.push_str("],\"hoops\":[");
        let hs: Vec<String> = m
            .hoops
            .iter()
            .map(|h| format!("[{},{},{},{}]", num(h.pos[0]), num(h.pos[1]), num(h.pos[2]), h.team))
            .collect();
        j.push_str(&hs.join(","));
        j.push_str("],\"events\":[");
        let es: Vec<String> =
            log.iter().map(|(t, k, text)| format!("[{},\"{}\",\"{}\"]", t / 2, k, text.replace('"', "'"))).collect();
        j.push_str(&es.join(","));
        j.push_str("],\"frames\":[");
        let fs: Vec<String> =
            frames.iter().map(|f| format!("[{}]", f.iter().map(|&v| num(v)).collect::<Vec<_>>().join(","))).collect();
        j.push_str(&fs.join(","));
        j.push_str("]}");
        std::fs::write(out, j).unwrap_or_else(|e| fail(e));
        eprintln!("wrote {}", out.display());
    }
}

/// Point `p` in `view`: the larger of its fractions of the half width and height from the centre (10 behind it).
fn framed(view: &crate::matchcam::View, p: qwc_render::glam::Vec3) -> f32 {
    let d = p - view.eye;
    let z = d.dot(view.dir);
    if z <= 0.0 {
        return 10.0;
    }
    let right = view.dir.cross(view.up).normalize_or_zero();
    let up = right.cross(view.dir).normalize_or_zero();
    let th = (view.fov * 0.5).to_radians().tan();
    let tv = (view.fov * 0.375).to_radians().tan();
    (d.dot(right) / (z * th)).abs().max((d.dot(up) / (z * tv)).abs())
}
