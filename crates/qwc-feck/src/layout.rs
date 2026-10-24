//! The text layout (`0x4dd8c0`, `docs/FECK_VM.md` "Text layout"): a text object's formatted string cut into line
//! records (runs of characters with a position and an optional colour), with word wrap, alignment, tab stops,
//! colour codes and inline sprites. Ported from the disassembly; **[T]** unless marked **[I]**, including the
//! original's arithmetic quirks.

/// What the layout needs from a font (`FONT`, loaded from `.ffn`).
pub trait Metrics {
    /// The glyph's advance (`0x500c00`): the 16-bit advance of a 16-byte glyph, the signed byte of a 12-byte one;
    /// 0 when the font has no such glyph.
    fn advance(&self, c: u16) -> i32;
    /// The kerning adjustment for `c` after `prev` (`0x500bc0`); 0 when there is none.
    fn kern(&self, c: u16, prev: u16) -> i32;
    /// The line height (`font +0x3c`, the header's two bytes at 0x12 added).
    fn line_height(&self) -> u16;
}

/// A loaded `.ffn` font (`FONT::Load` `0x4ff6b0`): the glyph is found by code (a binary search, `0x4ff010`); a
/// 16-byte glyph (flag `0x40000`) gives a signed 16-bit advance, a 12-byte one a signed byte; the line height is the
/// two signed bytes at 0x12 added.
impl Metrics for qwc_formats::ffn::Ffn<'_> {
    fn advance(&self, c: u16) -> i32 {
        let Some(g) = self.glyphs.iter().find(|g| g.code == c) else {
            return 0;
        };
        if self.flags & 0x40000 != 0 {
            g.advance as i16 as i32
        } else {
            g.advance as u8 as i8 as i32
        }
    }

    fn kern(&self, c: u16, prev: u16) -> i32 {
        qwc_formats::ffn::Ffn::kern(self, prev, c) as i32
    }

    fn line_height(&self) -> u16 {
        (self.line_a as i8 as i32 + self.line_b as i8 as i32) as u16
    }
}

/// The text object's settings the layout reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// The effect flags (`+0xc8`): 4 right-aligns, else 2 centres, else left.
    pub effect: u32,
    /// The width to fit in: the first ancestor with flag `0x30000` gives its width, else 512.
    pub avail: u16,
    /// The object's own x and y (`+0x08`, `+0x0c`). A line wraps when its width plus `x` reaches `avail`.
    pub x: f32,
    pub y: f32,
    /// The tab stops (`+0xd8`, `TEXt` properties; `0xffff` unused).
    pub tabs: [u16; 8],
}

/// One line record (`0x28` bytes: `+0x04` x, `+0x08` y, `+0x10` colour, `+0x20` count, `+0x22` start, `+0x24` has
/// colour). `start` and `count` index the text; they are `u16` and wrap like the original's.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub x: f32,
    pub y: f32,
    pub colour: Option<[f32; 4]>,
    pub start: u16,
    pub count: u16,
}

/// An inline sprite placed by the layout: the layer object `id` is moved to `(x, y)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Layout {
    /// In drawing order (the original's linked list).
    pub lines: Vec<Line>,
    /// `+0xd4`: the widest line (`x + width`).
    pub width: u16,
    /// `+0xd6`: one line height plus one per new line.
    pub height: u16,
    pub placed: Vec<Placed>,
}

/// A colour change or an inline sprite seen on the way (16 slots on the original's stack).
#[derive(Debug, Clone, Copy)]
struct Mark {
    sprite: Option<u32>,
    x: i32,
    w: i32,
    colour: [f32; 4],
    line: usize,
    index: u16,
    at_start: bool,
}

const MARKS: usize = 16;

/// `0x4de610`: a new record after the current one, taking the current colour.
fn new_line(arena: &mut Vec<Line>, next: &mut Vec<Option<usize>>, cur: &mut usize, on: bool, c: [f32; 4]) {
    arena.push(Line { x: 0.0, y: 0.0, colour: on.then_some(c), start: 0, count: 0 });
    next.push(next[*cur]);
    let n = arena.len() - 1;
    next[*cur] = Some(n);
    *cur = n;
}

/// Lay out `text` (UTF-16 with control codes, as the string manager gives it). `extent` measures an inline sprite
/// by its object id (width, height), `None` when the layer has no such object.
pub fn layout(text: &[u16], font: &dyn Metrics, extent: &dyn Fn(u32) -> Option<(f32, f32)>, p: Params) -> Layout {
    let unit = |k: usize| text.get(k).copied().unwrap_or(0);
    let lh = font.line_height();
    let space = font.advance(b's' as u16);
    let mut mode = if p.effect & 4 != 0 { 1 } else { p.effect & 2 };
    // the lines as an arena plus the list order
    let mut arena = vec![Line { x: 0.0, y: 0.0, colour: None, start: 0, count: 0 }];
    let mut next: Vec<Option<usize>> = vec![None];
    let mut cur = 0usize;
    let mut marks: Vec<Mark> = Vec::new();
    let (mut i, mut pos, mut start, mut last) = (0u32, 0usize, 0u32, 0u32);
    let (mut w, mut x, mut y, mut max, mut total) = (0i32, 0i32, 0u16, 0u16, lh);
    let (mut prev, mut sp, mut sp_w) = (0u16, 0u32, 0i32);
    let (mut colour_on, mut colour) = (false, [0.0f32; 4]);
    let mut at_start = true;
    // `0x4de5e0`, then the widest-line update
    let finish = |l: &mut Line, mode: u32, w: i32, max: &mut u16| {
        let w = w as u16 as i32;
        match mode {
            1 => l.x = (p.avail as i32 - w) as f32,
            2 => l.x = ((p.avail as i32 - w) / 2) as f32,
            _ => {}
        }
        if (*max as f32) < w as f32 + l.x {
            *max = (l.x as i32 + w) as u16;
        }
    };
    loop {
        let c = unit(pos);
        let mut done = false;
        // the glyph path's previous width, for the wrap
        let mut measured: Option<i32> = None;
        match c {
            0 => {
                arena[cur].count = (i - start) as u16;
                done = true;
                finish(&mut arena[cur], mode, w, &mut max);
                at_start = true;
            }
            1 if mode != 0 => {
                arena[cur].count = (i - start) as u16;
                finish(&mut arena[cur], mode, w, &mut max);
                x = 0;
                new_line(&mut arena, &mut next, &mut cur, colour_on, colour);
                arena[cur].y = y as f32;
                arena[cur].start = (i + 1) as u16;
                mode = 0;
                start = i + 1;
                (w, prev, sp, sp_w, at_start) = (0, 0, 0, 0, true);
            }
            2 | 3 => {
                let m = if c == 2 { 2 } else { 1 };
                if mode != m {
                    mode = m;
                    if i as u16 != start as u16 {
                        arena[cur].count = (i - start) as u16;
                        finish(&mut arena[cur], m, w, &mut max);
                        x = 0;
                        new_line(&mut arena, &mut next, &mut cur, colour_on, colour);
                        arena[cur].y = y as f32;
                        arena[cur].start = (i + 1) as u16;
                        start = i + 1;
                        (w, prev, sp, sp_w) = (0, 0, 0, 0);
                        // code 2 also marks the next character as a line start; code 3 does not
                        if c == 2 {
                            at_start = true;
                        }
                    }
                }
            }
            8 => {
                for (k, v) in colour.iter_mut().enumerate() {
                    *v = unit(pos + 1 + k) as f32 * (1.0 / 255.0);
                }
                if marks.len() < MARKS {
                    i += 4;
                    marks.push(Mark {
                        sprite: None,
                        x: x as u16 as i32,
                        w: 0,
                        colour,
                        line: cur,
                        index: i as u16,
                        at_start,
                    });
                }
                colour_on = true;
                pos += 4;
            }
            9 if mode == 0 => {
                arena[cur].count = (i - start) as u16;
                finish(&mut arena[cur], mode, w, &mut max);
                x = arena[cur].x as i32 + w as u16 as i32;
                new_line(&mut arena, &mut next, &mut cur, colour_on, colour);
                start = i + 1;
                arena[cur].start = start as u16;
                arena[cur].y = y as f32;
                if let Some(&t) = p.tabs.iter().find(|&&t| t != 0xffff && (x as u16) < t) {
                    x = t as i32;
                }
                arena[cur].x = (x as u16) as f32;
                (w, prev, sp, sp_w, at_start) = (0, 0, 0, 0, true);
            }
            10 => {
                if mode != 0 {
                    arena[cur].count = (i - start) as u16;
                    finish(&mut arena[cur], mode, w, &mut max);
                    new_line(&mut arena, &mut next, &mut cur, colour_on, colour);
                    start = i + 1;
                }
                x = 0;
                y = y.wrapping_add(lh);
                total = total.wrapping_add(lh);
                if mode != 0 {
                    arena[cur].x = 0.0;
                    arena[cur].start = (i + 1) as u16;
                    arena[cur].y = y as f32;
                }
                (w, prev, sp, sp_w, at_start) = (0, 0, 0, 0, true);
            }
            11 => {
                let id = (unit(pos + 1) as u32) << 16 | unit(pos + 2) as u32;
                pos += 2;
                // a full mark table or an unknown object skips the code without counting its two units in `i`
                if let Some((ew, _height)) = extent(id).filter(|_| marks.len() < MARKS) {
                    let ew = ew as i32;
                    // the original also stores the sprite's y centred in the line (`lh / 2 - eh / 2 + y`) but nothing
                    // reads it back: the sprite is placed at the line's y
                    marks.push(Mark {
                        sprite: Some(id),
                        x: x as u16 as i32,
                        w: ew,
                        colour: [0.0; 4],
                        line: cur,
                        index: i as u16,
                        at_start,
                    });
                    w += ew;
                    i += 2;
                }
            }
            4..=7 | 0xc | 0xe | 0xf | 1 | 9 => {}
            0x20 => {
                x += space;
                sp_w = w;
                measured = Some(w);
                w += space;
                prev = c;
                sp = i;
            }
            0x40 if pos != 0 => {
                // a space that does not break the line
                measured = Some(w);
                w += space;
                prev = c;
            }
            _ => {
                let k = if prev != 0 { font.kern(c, prev) } else { 0 };
                let a = font.advance(c) + k;
                x += a;
                measured = Some(w);
                w += a;
                prev = c;
            }
        }
        if let Some(old) = measured {
            at_start = false;
            // wrap when the width plus the object's x reaches the space available
            // (`fcompp` then `test ah, 0x41`: an unordered compare also counts as not reached)
            let fits = (p.avail as f32).partial_cmp(&((w as u16 as f32) + p.x));
            let reached = matches!(fits, Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal));
            // **[I]** a character alone on its line cannot move down (the original would loop for ever)
            let stuck = sp as u16 == 0 && last < start;
            if reached && !stuck {
                let (brk, line_w) = if sp as u16 == 0 { (last, old) } else { (sp, sp_w) };
                pos = brk as usize;
                arena[cur].count = (brk.wrapping_sub(start)) as u16;
                finish(&mut arena[cur], mode, line_w, &mut max);
                new_line(&mut arena, &mut next, &mut cur, colour_on, colour);
                x = 0;
                y = y.wrapping_add(lh);
                total = total.wrapping_add(lh);
                arena[cur].x = 0.0;
                arena[cur].y = y as f32;
                start = brk + (sp as u16 != 0) as u32;
                arena[cur].start = start as u16;
                i = brk;
                (w, prev, sp, sp_w, at_start) = (0, 0, 0, 0, true);
            }
        }
        pos += 1;
        last = i;
        i += 1;
        if done {
            break;
        }
    }
    let mut placed = Vec::new();
    // the marks: a colour or sprite at a line's start moves the line's start on; one inside a line splits it
    for m in 0..marks.len() {
        let r = marks[m];
        let l = r.line;
        if r.at_start {
            if let Some(id) = r.sprite {
                placed.push(Placed { id, x: arena[l].x, y: p.y + arena[l].y });
            }
            let line = &mut arena[l];
            line.count = line.count.wrapping_sub(3);
            line.start = line.start.wrapping_add(3);
            line.x += r.w as f32;
            if r.sprite.is_none() {
                line.count = line.count.wrapping_sub(2);
                line.start = line.start.wrapping_add(2);
                line.colour = Some(r.colour);
            }
            continue;
        }
        new_line(&mut arena, &mut next, &mut { l }, false, [0.0; 4]);
        let n = arena.len() - 1;
        let old = arena[l];
        let mut split = Line {
            x: (r.x + r.w) as f32 + old.x,
            y: old.y,
            colour: old.colour,
            start: r.index.wrapping_add(3),
            count: old.start.wrapping_sub(r.index).wrapping_add(old.count).wrapping_sub(3),
        };
        arena[l].count = r.index.wrapping_sub(old.start);
        match r.sprite {
            None => {
                split.count = split.count.wrapping_sub(2);
                split.colour = Some(r.colour);
                arena[l].count = arena[l].count.wrapping_sub(5);
            }
            Some(id) => placed.push(Placed { id, x: r.x as f32 + old.x, y: p.y + old.y }),
        }
        arena[n] = split;
        // the later marks on the split line move to the new record, their x made relative to it
        for later in &mut marks[m + 1..] {
            if later.line == l {
                later.x += r.w;
                later.x = (later.x as f32 - (split.x - old.x)) as i32;
                later.line = n;
            }
        }
    }
    let mut lines = Vec::new();
    let mut k = Some(0);
    while let Some(j) = k {
        lines.push(arena[j]);
        k = next[j];
    }
    Layout { lines, width: max, height: total, placed }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every glyph is 10 wide, 's' too; "AV" kerns by -2; lines are 20 high.
    struct Mono;

    impl Metrics for Mono {
        fn advance(&self, _: u16) -> i32 {
            10
        }
        fn kern(&self, c: u16, prev: u16) -> i32 {
            if (prev, c) == (b'A' as u16, b'V' as u16) {
                -2
            } else {
                0
            }
        }
        fn line_height(&self) -> u16 {
            20
        }
    }

    fn params(effect: u32, avail: u16) -> Params {
        Params { effect, avail, x: 0.0, y: 0.0, tabs: [0xffff; 8] }
    }

    fn run(s: &str, p: Params) -> Layout {
        let t: Vec<u16> = s.encode_utf16().collect();
        layout(&t, &Mono, &|id| (id == 0x0001_0002).then_some((16.0, 8.0)), p)
    }

    fn spans(l: &Layout) -> Vec<(f32, f32, u16, u16)> {
        l.lines.iter().map(|l| (l.x, l.y, l.start, l.count)).collect()
    }

    #[test]
    fn one_line_left_centred_right() {
        let l = run("AVB", params(0, 100));
        // A 10, V 10 - 2, B 10
        assert_eq!((spans(&l), l.width, l.height), (vec![(0.0, 0.0, 0, 3)], 28, 20));
        assert_eq!(spans(&run("AB", params(2, 100))), vec![(40.0, 0.0, 0, 2)]);
        assert_eq!(spans(&run("AB", params(4, 100))), vec![(80.0, 0.0, 0, 2)]);
    }

    #[test]
    fn wraps_at_the_last_space() {
        // "ab cd ef" at 50: "ab cd" is 50 wide, which reaches the limit at 'd'
        let l = run("ab cd ef", params(0, 50));
        assert_eq!(spans(&l), vec![(0.0, 0.0, 0, 2), (0.0, 20.0, 3, 2), (0.0, 40.0, 6, 2)]);
        assert_eq!((l.width, l.height), (20, 60));
    }

    #[test]
    fn wraps_inside_a_word() {
        // no space: the old line ends before the previous character and the new line starts at it, but only the
        // characters from the one that reached the limit are measured again
        let l = run("abcdef", params(0, 40));
        assert_eq!(spans(&l), vec![(0.0, 0.0, 0, 2), (0.0, 20.0, 2, 4)]);
    }

    #[test]
    fn newlines_split_only_aligned_text() {
        assert_eq!(spans(&run("ab\ncd", params(0, 100))), vec![(0.0, 0.0, 0, 5)]);
        assert_eq!(spans(&run("ab\ncd", params(2, 100))), vec![(40.0, 0.0, 0, 2), (40.0, 20.0, 3, 2)]);
        assert_eq!(run("ab\ncd", params(0, 100)).height, 40);
    }

    #[test]
    fn tab_stops() {
        let mut p = params(0, 200);
        p.tabs[0] = 15;
        p.tabs[1] = 60;
        // after "ab" the pen is at 20: the first stop beyond it is 60
        assert_eq!(spans(&run("ab\tc", p)), vec![(0.0, 0.0, 0, 2), (60.0, 0.0, 3, 1)]);
    }

    #[test]
    fn colour_at_a_line_start() {
        let mut t: Vec<u16> = vec![8, 255, 0, 0, 255];
        t.extend("ab".encode_utf16());
        let l = layout(&t, &Mono, &|_| None, params(0, 100));
        assert_eq!(spans(&l), vec![(0.0, 0.0, 5, 2)]);
        assert_eq!(l.lines[0].colour, Some([1.0, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn inline_sprite_splits_the_line() {
        let mut t: Vec<u16> = "a".encode_utf16().collect();
        t.extend([11, 1, 2]);
        t.extend("b".encode_utf16());
        let l = layout(&t, &Mono, &|id| (id == 0x0001_0002).then_some((16.0, 8.0)), params(0, 100));
        // "a" then the sprite (16 wide, centred in the 20 high line) then "b" after it
        assert_eq!(spans(&l), vec![(0.0, 0.0, 0, 1), (26.0, 0.0, 4, 1)]);
        assert_eq!(l.placed, vec![Placed { id: 0x0001_0002, x: 10.0, y: 0.0 }]);
        // an unknown sprite takes no room
        let l = layout(&t, &Mono, &|_| None, params(0, 100));
        assert_eq!(l.lines.len(), 1);
    }
}
