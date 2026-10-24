use super::*;

fn close(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
}

fn image(name: &str, texture: &str, position: [f32; 2], size: [f32; 2]) -> Element {
    Element {
        name: name.into(),
        kind: Kind::Image(Image { texture: texture.into(), ..Default::default() }),
        position,
        size,
        ..Default::default()
    }
}

/// The draws of the element named by `names` from the top.
fn draws<'a>(scene: &Scene, f: &'a Frame, names: &[&str]) -> Vec<&'a Draw> {
    let mut path = Vec::new();
    let mut list = &scene.elements;
    for n in names {
        let i = list.iter().position(|e| e.name == *n).unwrap_or_else(|| panic!("no {n}"));
        path.push(i);
        list = &list[i].children;
    }
    f.draws.iter().filter(|d| f.placed[d.element].path == path).collect()
}

#[test]
fn the_match_scene_reads_and_writes_back_the_same() {
    let scene = Scene::from_ron(MATCH).unwrap();
    assert_eq!(Scene::from_ron(&scene.to_ron()).unwrap(), scene);
    assert!(scene.to_ron().starts_with("#![enable(unwrap_variant_newtypes)]"));
}

#[test]
fn the_challenge_scene_reads_and_writes_back_as_the_editor_saves_it() {
    let scene = Scene::from_ron(CHALLENGE).unwrap();
    // RON ends its lines as the platform does, and so may the checkout
    assert_eq!(scene.to_ron().replace("\r\n", "\n"), CHALLENGE.replace("\r\n", "\n"));
}

#[test]
fn a_repeated_element_draws_a_copy_per_count_each_a_step_on_with_its_index() {
    let mut row = image("Row", "{b.#}", [10.0, 20.0], [8.0, 8.0]);
    row.repeat = Repeat { count: "n".into(), max: 4, step: [12.0, 1.0] };
    row.show_if = "on.#".into();
    let scene = Scene { scale: ScaleRule::Height, elements: vec![row], ..Default::default() };
    let mut v = Values::default();
    for (i, t) in ["red", "green", "blue", "grey", "pink"].iter().enumerate() {
        v.set(&format!("b.{i}"), Value::Text(t.to_string()));
        v.set(&format!("on.{i}"), Value::Flag(i != 1));
    }
    v.set("n", Value::Scalar(3.0));
    let f = scene.evaluate([640.0, 480.0], &v, false);
    // copy 1 is hidden by its own `on.1`; every copy is the element's in the placed list
    let got: Vec<(&str, [f32; 4])> = f.draws.iter().map(|d| (d.texture.as_str(), d.rect)).collect();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].0, "red");
    assert!(close(&got[0].1, &[10.0, 20.0, 18.0, 28.0]));
    assert_eq!(got[1].0, "blue");
    assert!(close(&got[1].1, &[34.0, 22.0, 42.0, 30.0]));
    assert_eq!(f.placed.len(), 3);
    assert!(f.placed.iter().all(|p| p.path == [0]));
    // held at the most
    v.set("n", Value::Scalar(9.0));
    assert_eq!(scene.evaluate([640.0, 480.0], &v, false).placed.len(), 4);
    // a stretched axis moves both insets
    let mut bar =
        Element { anchors: Anchors { min: [0.0, 0.0], max: [1.0, 0.0] }, ..image("", "t", [0.0; 2], [0.0; 2]) };
    bar.size = [0.0, 10.0];
    bar.repeat = Repeat { count: "n".into(), max: 2, step: [5.0, 0.0] };
    let f = Scene { scale: ScaleRule::Height, elements: vec![bar], ..Default::default() }.evaluate(
        [640.0, 480.0],
        &v,
        false,
    );
    assert!(close(&f.draws[1].rect, &[5.0, 0.0, 645.0, 10.0]));
}

#[test]
fn the_challenge_scene_puts_a_button_per_pass_the_clock_at_the_right_and_the_countdown_in_the_middle() {
    let scene = Scene::from_ron(CHALLENGE).unwrap();
    let mut v = Values::defaults(&scene);
    let f = scene.evaluate([640.0, 480.0], &v, false);
    let buttons = draws(&scene, &f, &["Passes"]);
    assert_eq!(buttons.len(), 8);
    for (i, b) in buttons.iter().enumerate() {
        let x = 16.0 + 34.0 * i as f32;
        assert!(close(&b.rect, &[x, 12.0, x + 30.0, 42.0]), "{i}: {:?}", b.rect);
    }
    assert_eq!(buttons[2].texture, "Challenge_button_yellow");
    // the clock's three numerals, the last 14 units from the right (as the code had them)
    let clock = draws(&scene, &f, &["Clock"]);
    assert_eq!(
        clock.iter().map(|d| d.texture.as_str()).collect::<Vec<_>>(),
        ["QWC_GUI_Numerals0_execversion", "QWC_GUI_Numerals8_execversion", "QWC_GUI_Numerals7_execversion"]
    );
    assert!(close(&clock[0].rect, &[640.0 - 14.0 - 2.0 * 37.4 - 54.4, 2.0, 640.0 - 14.0 - 2.0 * 37.4, 56.4]));
    assert!(draws(&scene, &f, &["Countdown"]).is_empty());
    v.set("counting", Value::Flag(true));
    v.set("countdown", Value::Scalar(2.0));
    let f = scene.evaluate([640.0, 480.0], &v, false);
    let n = draws(&scene, &f, &["Countdown"]);
    assert_eq!(n.len(), 1);
    assert_eq!(n[0].texture, "QWC_GUI_Numerals2_execversion");
    assert!(close(&n[0].rect, &[256.0, 176.0, 384.0, 304.0]));
    // on a wide screen the clock keeps to the right edge and the buttons to the left
    let f = scene.evaluate([1920.0, 1080.0], &v, false);
    let k = 1080.0 / 480.0;
    assert!(close(&draws(&scene, &f, &["Passes"])[0].rect, &[16.0 * k, 12.0 * k, 46.0 * k, 42.0 * k]));
    assert!((draws(&scene, &f, &["Clock"])[2].rect[2] - (1920.0 - 14.0 * k)).abs() < 1e-2);
}

#[test]
fn a_centred_element_stays_centred_on_any_screen() {
    let e = Element {
        anchors: Anchors::point(0.5, 0.5),
        alignment: [0.5, 0.5],
        size: [100.0, 50.0],
        ..image("", "t", [0.0; 2], [0.0; 2])
    };
    let scene = Scene { elements: vec![e], ..Default::default() };
    let f = scene.evaluate([640.0, 480.0], &Values::default(), false);
    assert!(close(&f.draws[0].rect, &[270.0, 215.0, 370.0, 265.0]));
    // the shorter side scales: 1080 / 480
    let f = scene.evaluate([1920.0, 1080.0], &Values::default(), false);
    assert!(close(&f.scale, &[2.25, 2.25]));
    assert!(close(&f.canvas, &[1920.0 / 2.25, 480.0]));
    assert!(close(&f.draws[0].rect, &[960.0 - 112.5, 540.0 - 56.25, 960.0 + 112.5, 540.0 + 56.25]));
    // stretched, as the original's HUD was
    let scene = Scene { scale: ScaleRule::Stretch, ..scene };
    let f = scene.evaluate([1920.0, 1080.0], &Values::default(), false);
    assert!(close(&f.draws[0].rect, &[960.0 - 150.0, 540.0 - 56.25, 960.0 + 150.0, 540.0 + 56.25]));
}

#[test]
fn stretched_axes_keep_their_insets_and_corners_their_offsets() {
    let bar =
        Element { anchors: Anchors { min: [0.0, 0.0], max: [1.0, 0.0] }, ..image("", "t", [10.0, 5.0], [20.0, 30.0]) };
    let corner = Element {
        anchors: Anchors::point(1.0, 1.0),
        alignment: [1.0, 1.0],
        ..image("", "t", [-8.0, -8.0], [16.0, 16.0])
    };
    let scene = Scene { scale: ScaleRule::Height, elements: vec![bar, corner], ..Default::default() };
    let f = scene.evaluate([800.0, 480.0], &Values::default(), false);
    assert!(close(&f.draws[0].rect, &[10.0, 5.0, 780.0, 35.0]));
    assert!(close(&f.draws[1].rect, &[776.0, 456.0, 792.0, 472.0]));
}

#[test]
fn drivers_add_their_keys_linearly_and_hold_beyond_the_ends() {
    let d = Driver { value: "v".into(), property: Property::X, keys: vec![[0.0, 0.0], [0.5, 10.0], [1.0, 100.0]] };
    assert!(close(&[d.at(-1.0), d.at(0.25), d.at(0.75), d.at(2.0)], &[0.0, 5.0, 55.0, 100.0]));
    let unsorted = Driver { keys: vec![[1.0, 100.0], [0.0, 0.0]], ..d.clone() };
    assert!(close(&[unsorted.at(0.5)], &[50.0]));
    let mut e = image("", "t", [10.0, 0.0], [10.0, 10.0]);
    e.drivers = vec![d, Driver { value: "v".into(), property: Property::Scale, keys: vec![[0.0, 0.0], [1.0, 1.0]] }];
    let scene = Scene { scale: ScaleRule::Stretch, elements: vec![e], ..Default::default() };
    let mut values = Values::default();
    values.set("v", Value::Scalar(1.0));
    // moved 100 and doubled about its middle
    let f = scene.evaluate([640.0, 480.0], &values, false);
    assert!(close(&f.draws[0].rect, &[105.0, -5.0, 125.0, 15.0]));
}

#[test]
fn drivers_move_an_element_that_stretches_and_anchors_move_it_by_a_share_of_its_parent() {
    // across the top, 100 in from each side: a move shifts both insets, a size moves the far one
    let mut e = Element {
        anchors: Anchors { min: [0.0, 0.0], max: [1.0, 0.0] },
        ..image("", "t", [100.0, 0.0], [100.0, 10.0])
    };
    e.drivers = vec![
        Driver { value: "v".into(), property: Property::X, keys: vec![[0.0, 0.0], [1.0, 50.0]] },
        Driver { value: "w".into(), property: Property::Width, keys: vec![[0.0, 0.0], [1.0, 20.0]] },
    ];
    let scene = Scene { scale: ScaleRule::Height, elements: vec![e], ..Default::default() };
    let at = |v: f32, w: f32, width: f32| {
        let mut values = Values::default();
        values.set("v", Value::Scalar(v));
        values.set("w", Value::Scalar(w));
        scene.evaluate([width, 480.0], &values, false).draws[0].rect
    };
    assert!(close(&at(0.0, 0.0, 640.0), &[100.0, 0.0, 540.0, 10.0]));
    assert!(close(&at(1.0, 0.0, 640.0), &[150.0, 0.0, 590.0, 10.0]));
    assert!(close(&at(0.0, 1.0, 640.0), &[100.0, 0.0, 560.0, 10.0]));
    // a point anchor driven from 0 to 1 rides across its parent, whatever the parent's width
    let mut rider = image("", "t", [-5.0, 0.0], [10.0, 10.0]);
    rider.drivers = vec![Driver { value: "v".into(), property: Property::AnchorX, keys: vec![[0.0, 0.0], [1.0, 1.0]] }];
    let track = Element {
        anchors: Anchors { min: [0.0, 0.0], max: [1.0, 0.0] },
        children: vec![rider],
        ..image("", "", [100.0, 0.0], [100.0, 10.0])
    };
    let scene = Scene { scale: ScaleRule::Height, elements: vec![track], ..Default::default() };
    for width in [640.0, 1280.0] {
        let mut values = Values::default();
        values.set("v", Value::Scalar(0.25));
        let f = scene.evaluate([width, 480.0], &values, false);
        let x = 100.0 + 0.25 * (width - 200.0) - 5.0;
        assert!(close(&f.draws.last().unwrap().rect, &[x, 0.0, x + 10.0, 10.0]));
    }
}

#[test]
fn a_mirrored_group_mirrors_its_images_but_a_number_still_reads_left_to_right() {
    let mut group = Element { size: [200.0, 100.0], scale: [-1.0, 1.0], ..Default::default() };
    group.children.push(image("", "t", [0.0, 0.0], [50.0, 20.0]));
    group.children.push(Element {
        kind: Kind::Number(Number {
            value: "n".into(),
            digits: 3,
            numerals: "n{d}".into(),
            glyph: [10.0, 10.0],
            step: 10.0,
            justify: 0.0,
        }),
        ..image("", "", [0.0, 50.0], [30.0, 10.0])
    });
    let scene = Scene { scale: ScaleRule::Stretch, elements: vec![group], ..Default::default() };
    let mut values = Values::default();
    values.set("n", Value::Scalar(12.0));
    let f = scene.evaluate([640.0, 480.0], &values, false);
    assert!(close(&f.draws[0].rect, &[150.0, 0.0, 200.0, 20.0]));
    assert!(close(&f.draws[0].uv, &[1.0, 0.0, 0.0, 1.0]));
    let n: Vec<_> = f.draws[1..].iter().map(|d| (d.texture.as_str(), d.rect[0])).collect();
    assert_eq!(n, [("n0", 170.0), ("n1", 180.0), ("n2", 190.0)]);
}

#[test]
fn fills_show_part_of_the_box_from_their_edge() {
    let fill = |from, crop| {
        let mut e = image("", "t", [0.0, 0.0], [100.0, 40.0]);
        e.kind = Kind::Image(Image { texture: "t".into(), fill: 0.25, fill_from: from, crop, ..Default::default() });
        let scene = Scene { scale: ScaleRule::Stretch, elements: vec![e], ..Default::default() };
        let d = scene.evaluate([640.0, 480.0], &Values::default(), false).draws.remove(0);
        (d.rect, d.uv)
    };
    let (r, uv) = fill(Edge::Left, true);
    assert!(close(&r, &[0.0, 0.0, 25.0, 40.0]) && close(&uv, &[0.0, 0.0, 0.25, 1.0]));
    let (r, uv) = fill(Edge::Right, true);
    assert!(close(&r, &[75.0, 0.0, 100.0, 40.0]) && close(&uv, &[0.75, 0.0, 1.0, 1.0]));
    let (r, uv) = fill(Edge::Bottom, false);
    assert!(close(&r, &[0.0, 30.0, 100.0, 40.0]) && close(&uv, &[0.0, 0.0, 1.0, 1.0]));
    // nothing at all shows nothing
    let mut e = image("", "t", [0.0, 0.0], [100.0, 40.0]);
    e.kind = Kind::Image(Image { texture: "t".into(), fill: 0.0, ..Default::default() });
    let scene = Scene { elements: vec![e], ..Default::default() };
    assert!(scene.evaluate([640.0, 480.0], &Values::default(), false).draws.is_empty());
}

#[test]
fn show_if_carries_to_children_and_the_editors_eye_only_hides_there() {
    let mut group = Element { show_if: "!off".into(), ..Default::default() };
    group.children.push(image("", "t", [0.0; 2], [10.0, 10.0]));
    group.children.push(Element { hidden: true, ..image("", "t", [0.0; 2], [10.0, 10.0]) });
    let scene = Scene { elements: vec![group], ..Default::default() };
    let mut values = Values::default();
    assert_eq!(scene.evaluate([640.0, 480.0], &values, false).draws.len(), 2);
    assert_eq!(scene.evaluate([640.0, 480.0], &values, true).draws.len(), 1);
    values.set("off", Value::Flag(true));
    let f = scene.evaluate([640.0, 480.0], &values, false);
    assert!(f.draws.is_empty() && f.placed.iter().all(|p| !p.shown));
}

#[test]
fn draws_go_by_layer_then_in_the_scenes_order() {
    let layered = |name: &str, layer| Element { layer, ..image(name, name, [0.0; 2], [1.0, 1.0]) };
    let scene = Scene {
        elements: vec![layered("a", 54), layered("b", 50), layered("c", 54), layered("d", 52)],
        ..Default::default()
    };
    let f = scene.evaluate([640.0, 480.0], &Values::default(), false);
    let order: Vec<_> = f.draws.iter().map(|d| d.texture.as_str()).collect();
    assert_eq!(order, ["b", "d", "a", "c"]);
}

#[test]
fn values_substitute_into_textures_and_tint() {
    let mut e = image("", "teams/{team}/bar", [0.0; 2], [1.0, 1.0]);
    e.tint = "colour".into();
    e.colour = [0.5, 1.0, 1.0, 1.0];
    e.opacity = 0.5;
    let scene = Scene { elements: vec![e], ..Default::default() };
    let mut values = Values::default();
    values.set("team", Value::Text("England".into()));
    values.set("colour", Value::Colour([1.0, 0.5, 0.0, 1.0]));
    let d = &scene.evaluate([640.0, 480.0], &values, false).draws[0];
    assert_eq!(d.texture, "teams/England/bar");
    assert!(close(&d.colour, &[0.5, 0.5, 0.0, 0.5]));
    assert_eq!(texture_path(&d.texture), "teams/England/bar.fsh");
    assert_eq!(texture_path("topbar"), "root/HUD/topbar.fsh");
}

#[test]
fn anchors_and_alignment_change_without_moving_the_box() {
    let mut e = image("", "t", [30.0, 40.0], [100.0, 50.0]);
    let parent = [640.0, 480.0];
    let before = e.local_box(parent);
    e.set_anchors_keeping_box(Anchors { min: [0.5, 0.0], max: [1.0, 0.0] }, parent);
    assert!(close(&e.local_box(parent), &before));
    assert!(close(&e.position, &[30.0 - 320.0, 40.0]) && close(&e.size, &[640.0 - 130.0, 50.0]));
    e.set_anchors_keeping_box(Anchors::point(1.0, 1.0), parent);
    e.set_alignment_keeping_box([0.5, 0.5], parent);
    assert!(close(&e.local_box(parent), &before));
    assert!(close(&e.size, &[100.0, 50.0]));
}

/// The match scene with these powers on this screen, and the Seeker game on or not.
fn match_frame(scene: &Scene, screen: [f32; 2], powers: [f32; 2], seeker: bool) -> Frame {
    let mut values = Values::defaults(scene);
    values.set("power.left", Value::Scalar(powers[0]));
    values.set("power.right", Value::Scalar(powers[1]));
    values.set("seeker_game", Value::Flag(seeker));
    values.set("score.right", Value::Scalar(123.0));
    scene.evaluate(screen, &values, false)
}

const SCREENS: [[f32; 2]; 4] = [[640.0, 480.0], [1920.0, 1080.0], [2560.0, 1080.0], [1280.0, 1024.0]];

/// The stock scene is the HUD as measured from the original's screen (a pack's `hud/match.ron` stands in for it).
#[test]
fn the_match_scene_at_640_by_480_is_the_measured_layout() {
    let scene = Scene::from_ron(MATCH).unwrap();
    let mut values = Values::defaults(&scene);
    for (name, v) in [("power.left", 0.5), ("power.right", 0.25), ("score.left", 7.0), ("score.right", 123.0)] {
        values.set(name, Value::Scalar(v));
    }
    values.set("cards.right", Value::Flag(true));
    let f = scene.evaluate([640.0, 480.0], &values, false);
    let one = |names: &[&str]| draws(&scene, &f, names)[0].clone();
    let span = 640.0 - 2.0 * 177.5 - 2.0 * (45.0 / 64.0 * 41.4);
    let fill = one(&["Top bar", "Home", "Box fill"]);
    assert!(close(&fill.rect, &[64.4, 47.2, 177.5, 86.1]) && close(&fill.uv, &[0.0, 0.0, 71.0 / 128.0, 43.0 / 64.0]));
    assert_eq!(fill.layer, 0x32);
    let frame = one(&["Top bar", "Away", "Box frame"]);
    assert!(close(&frame.rect, &[462.5, 47.2, 575.6, 86.1]) && close(&frame.uv, &[0.25, 0.0, 0.0, 50.0 / 64.0]));
    let bar = one(&["Top bar", "Home", "Track", "Bar"]);
    assert_eq!(bar.texture, "teams/England/QWC_GUI_SnitchBar");
    assert!(close(&bar.rect, &[177.5, 55.6, 177.5 + 0.5 * span, 74.2]), "{:?}", bar.rect);
    assert!(close(&bar.uv, &[0.0, 0.0, 0.5 * 190.0 / 256.0, 0.75]));
    let bar = one(&["Top bar", "Away", "Track", "Bar"]);
    assert!(close(&bar.rect, &[462.5 - 0.25 * span, 55.6, 462.5, 74.2]), "{:?}", bar.rect);
    assert!(close(&bar.uv, &[0.25 * 190.0 / 256.0, 0.0, 0.0, 0.75]));
    // the Snitch halves on the bars' tips, unmirrored
    let (sw, ends) = (41.4, [177.5 + 0.5 * span, 462.5 - 0.25 * span]);
    let left = one(&["Top bar", "Home", "Track", "Snitch half"]);
    assert!(close(&left.rect, &[ends[0] - 9.0 / 64.0 * sw, 38.9, ends[0] + 55.0 / 64.0 * sw, 90.9]), "{:?}", left.rect);
    let right = one(&["Top bar", "Away", "Track", "Snitch half"]);
    let want = [ends[1] - 51.0 / 64.0 * sw, 38.9, ends[1] + 13.0 / 64.0 * sw, 90.9];
    assert!(close(&right.rect, &want), "{:?}", right.rect);
    assert!(close(&right.uv, &[0.0, 0.0, 1.0, 1.0]));
    // the scores centred in their boxes, reading left to right
    let score: Vec<_> =
        draws(&scene, &f, &["Top bar", "Away", "Score"]).iter().map(|d| (d.texture.clone(), d.rect[0])).collect();
    let centre = 640.0 - (64.4 + 177.5) / 2.0;
    for (i, (t, x)) in score.iter().enumerate() {
        assert_eq!(*t, format!("QWC_GUI_Numerals{}_execversion", i + 1));
        assert!(close(&[*x], &[centre + (i as f32 - 1.0) * 34.3 - 44.7 / 2.0]), "{x}");
    }
    let left: Vec<_> = draws(&scene, &f, &["Top bar", "Home", "Score"]).iter().map(|d| d.texture.clone()).collect();
    assert_eq!(left[2], "QWC_GUI_Numerals7_execversion");
    // the moves under the boxes, the dodge first on both sides
    for (names, x0) in [(["Top bar", "Home", "Moves", "Dodge"], 64.4), (["Top bar", "Away", "Moves", "Dodge"], 483.6)] {
        assert!(close(&one(&names).rect, &[x0, 90.1, x0 + 28.0, 118.1]));
    }
    // in the Seeker game and the race: the glow where the tips meet, the gauges at the sides
    values.set("seeker_game", Value::Flag(true));
    values.set("race", Value::Flag(true));
    values.set("power.right", Value::Scalar(0.5));
    values.set("seeker.right.level", Value::Scalar(0.5));
    let f = scene.evaluate([640.0, 480.0], &values, false);
    let one = |names: &[&str]| draws(&scene, &f, names)[0].clone();
    let c = (ends[0] + 462.5 - 0.5 * span) / 2.0;
    for side in ["Home", "Away"] {
        let glow = one(&["Top bar", side, "Track", "Snitch glow"]);
        assert!(close(&glow.rect, &[c - 52.0, 12.9, c + 52.0, 116.9]), "{side}: {:?}", glow.rect);
    }
    assert!(close(&one(&["Home gauge", "Frame"]).rect, &[16.0, 150.0, 64.0, 342.0]));
    let level = one(&["Away gauge", "Level"]);
    assert!(close(&level.rect, &[588.0, 213.0, 601.5, 273.0]) && close(&level.uv, &[0.0, 0.0, 1.0, 1.0]));
    // on a wide screen the bar keeps its shape in the middle and the gauges keep to the sides
    let f = scene.evaluate([1920.0, 1080.0], &values, false);
    let k = 2.25;
    let x0 = (1920.0 / k - 640.0) / 2.0;
    let fill = draws(&scene, &f, &["Top bar", "Home", "Box fill"])[0].clone();
    assert!(close(&fill.rect, &[(x0 + 64.4) * k, 47.2 * k, (x0 + 177.5) * k, 86.1 * k]), "{:?}", fill.rect);
    let gauge = draws(&scene, &f, &["Away gauge", "Frame"])[0].clone();
    let want = [1920.0 - 64.0 * k, 150.0 * k, 1920.0 - 16.0 * k, 342.0 * k];
    assert!(close(&gauge.rect, &want), "{:?}", gauge.rect);
}

#[test]
fn the_match_scenes_snitch_halves_ride_their_bars_and_meet_on_any_screen() {
    let scene = Scene::from_ron(MATCH).unwrap();
    let units = |f: &Frame, names: &[&str]| {
        let d = draws(&scene, f, names);
        assert_eq!(d.len(), 1, "{names:?}");
        let k = f.scale[0];
        d[0].rect.map(|v| v / k)
    };
    let side = |s: &'static str, what: &'static str| ["Top bar", s, "Track", what];
    for screen in SCREENS {
        for p in [0.1, 0.35, 0.5, 0.8, 0.95] {
            let f = match_frame(&scene, screen, [p, 1.0 - p], false);
            let r = |s, w| units(&f, &side(s, w));
            let (bar, left, right, away) =
                (r("Home", "Bar"), r("Home", "Snitch half"), r("Away", "Snitch half"), r("Away", "Bar"));
            // each bar's tip under its half, so neither leaves a gap
            assert!((left[0]..=left[2]).contains(&bar[2]), "{screen:?} at {p}: the home bar's tip off its half");
            assert!((right[0]..=right[2]).contains(&away[0]), "{screen:?} at {p}: the away bar's tip off its half");
            // the halves meet, overlapping without passing each other (closer on wider screens)
            assert!(left[0] <= right[0] && left[2] > right[0], "{screen:?} at {p}: the halves {left:?} {right:?}");
            // in the Seeker game the halves leave the bar for the world, and the glows are on where they met
            let f = match_frame(&scene, screen, [p, 1.0 - p], true);
            assert!(draws(&scene, &f, &side("Home", "Snitch half")).is_empty());
            assert!(draws(&scene, &f, &side("Away", "Snitch half")).is_empty());
            let glow = units(&f, &["Top bar", "Home", "Track", "Snitch glow"]);
            let between = (left[0] + left[2] + right[0] + right[2]) / 4.0;
            assert!(((glow[0] + glow[2]) / 2.0 - between).abs() < 1.5, "{screen:?} at {p}: the glow off the Snitch");
        }
    }
    // no glow until the Seeker game
    let f = match_frame(&scene, SCREENS[0], [0.3, 0.3], false);
    assert!(draws(&scene, &f, &["Top bar", "Home", "Track", "Snitch glow"]).is_empty());
}

#[test]
fn the_match_scenes_scores_read_left_to_right_on_both_sides() {
    let scene = Scene::from_ron(MATCH).unwrap();
    for screen in SCREENS {
        let f = match_frame(&scene, screen, [0.2, 0.3], false);
        let mut score: Vec<_> = draws(&scene, &f, &["Top bar", "Away", "Score"]).into_iter().cloned().collect();
        let order: Vec<_> = score.iter().map(|d| d.rect[0]).collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]));
        score.sort_by(|a, b| a.rect[0].total_cmp(&b.rect[0]));
        let digits: Vec<_> = score.iter().map(|d| d.texture.as_str()).collect();
        assert_eq!(
            digits,
            ["QWC_GUI_Numerals1_execversion", "QWC_GUI_Numerals2_execversion", "QWC_GUI_Numerals3_execversion"]
        );
    }
}
