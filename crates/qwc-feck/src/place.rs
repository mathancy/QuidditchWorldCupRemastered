//! Where the front end's 3D objects stand (`docs/FECK_VM.md` "Models and puppets"): the matrices the renderer
//! draws its models with (`0x4e4ff0`) and the ones a puppet puts on its character's root node (`0x4dc4d0`, and
//! with a spline turned it the blended record's, `0x4e8310`). Matrices are as Direct3D keeps them: 16 floats row by
//! row, for row vectors (`v · M`), the translation in the last row. They are in the world the screen's camera
//! looks at (the stadium's), before the view.

use crate::draw::{Model, Puppet};
use crate::frontend::FrontEnd;
use crate::spline;

pub type Matrix = [f32; 16];

pub const IDENTITY: Matrix = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];

/// `a · b` (`0x401680`, `D3DXMatrixMultiply`).
pub fn mul(a: &Matrix, b: &Matrix) -> Matrix {
    std::array::from_fn(|k| {
        let (r, c) = (k / 4, k % 4);
        (0..4).map(|i| a[4 * r + i] * b[4 * i + c]).sum()
    })
}

/// `D3DXMatrixRotationX`, `Y` and `Z` (`0x571c65`, `0x571d01`, `0x571d9e`: the D3DX table's entries
/// `0x5c6a08`-`0x5c6a10`).
pub fn rotation_x(a: f32) -> Matrix {
    let (s, c) = a.sin_cos();
    [1.0, 0.0, 0.0, 0.0, 0.0, c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0]
}

pub fn rotation_y(a: f32) -> Matrix {
    let (s, c) = a.sin_cos();
    [c, 0.0, -s, 0.0, 0.0, 1.0, 0.0, 0.0, s, 0.0, c, 0.0, 0.0, 0.0, 0.0, 1.0]
}

pub fn rotation_z(a: f32) -> Matrix {
    let (s, c) = a.sin_cos();
    [c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]
}

/// `D3DXMatrixTranslation` (`0x571bd5`).
pub fn translation(p: [f32; 3]) -> Matrix {
    let mut m = IDENTITY;
    m[12..15].copy_from_slice(&p);
    m
}

/// `0x4213f0`: the rotation of angles `(x, y, z)` (radians), rows `cz cy, -sz cy, sy`;
/// `sy sx cz + sz cx, cz cx - sx sz sy, -sx cy`; `sz sx - sy cz cx, sz sy cx + sx cz, cy cx`. For row vectors that
/// turns about X by `-x`, then Y by `-y`, then Z by `-z`.
pub fn euler(a: [f32; 3]) -> Matrix {
    let ([sx, sy, sz], [cx, cy, cz]) = (a.map(f32::sin), a.map(f32::cos));
    [
        cz * cy,
        -(sz * cy),
        sy,
        0.0,
        sy * sx * cz + sz * cx,
        cz * cx - sx * (sz * sy),
        -(sx * cy),
        0.0,
        sz * sx - sy * cz * cx,
        sz * sy * cx + sx * cz,
        cy * cx,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ]
}

/// A rotation `rot` (a quaternion's matrix, `0x42c6e0`) placed at `pos`.
fn placed(rot: Matrix, pos: [f32; 3]) -> Matrix {
    let mut m = rot;
    m[12..15].copy_from_slice(&pos);
    m[15] = 1.0;
    m
}

impl Model {
    /// `0x4e4ff0` in the screen's region: turned about Y by minus its y angle, then X by minus x, then Z by minus z
    /// (`+0x18`-`+0x20`); or, with a rotation mode (`+0x34`, which a model spline sets), by its quaternion
    /// (`+0x24`, the object's `+0xc4`, `0x42c6e0`); then moved to its position. The renderer then multiplies in the
    /// view (`+0x40` of its camera).
    pub fn world(&self) -> Matrix {
        let rot = if self.rotation_mode == 0 {
            let [x, y, z] = self.rot;
            mul(&mul(&rotation_y(-y), &rotation_x(-x)), &rotation_z(-z))
        } else {
            spline::matrix(self.extra)
        };
        mul(&rot, &translation([self.pos[0], self.pos[1], self.pos[2]]))
    }
}

impl Puppet {
    /// `0x4e8310` for a puppet record: a puppet a spline has turned (`+0x131`) has its character's root node set to
    /// the blended record, its quaternion's matrix at its position (the spline's while it runs, else its own position
    /// unrotated). Otherwise the character stays where its update put it ([`FrontEnd::puppet_root`], as the record
    /// keeps it: `stands`); `None` without a character.
    pub fn root(&self) -> Option<Matrix> {
        if !self.relative {
            return self.stands;
        }
        Some(placed(spline::matrix(self.rotation), [self.pos[0], self.pos[1], self.pos[2]]))
    }
}

impl FrontEnd {
    /// `0x4dc4d0`, a puppet's update (its type's `+0x1c`) when its character is loaded: on a spline (`+0x130`) the
    /// spline's quaternion (`+0xcc`) at its position (`+0xdc`); otherwise its angles (`+0x28`-`+0x30`, [`euler`])
    /// at its position, first turned half round about Y with flag `0x400000` (`RY(π) · M`, `0x433590`). It becomes
    /// the character's root node matrix (`0x4689a0`). `None` for an object without a character.
    pub fn puppet_root(&self, i: usize) -> Option<Matrix> {
        if !self.puppets.contains_key(&i) {
            return None;
        }
        let o = &self.objects[i];
        let f = |off: u16| f32::from_bits(o.slot(off));
        if o.slot(0x130) & 0xff != 0 {
            let q = [f(0xcc), f(0xd0), f(0xd4), f(0xd8)];
            return Some(placed(spline::matrix(q), [f(0xdc), f(0xe0), f(0xe4)]));
        }
        let m = placed(euler(o.rot), [o.pos[0], o.pos[1], o.pos[2]]);
        Some(if o.flags & 0x40_0000 != 0 { mul(&rotation_y(std::f32::consts::PI), &m) } else { m })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: &Matrix, v: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|c| v[0] * m[c] + v[1] * m[4 + c] + v[2] * m[8 + c] + m[12 + c])
    }

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn rotations_turn_as_direct3d_does() {
        let q = std::f32::consts::FRAC_PI_2;
        // D3DX's rotations for row vectors: Y takes +x to -z, X takes +y to +z, Z takes +x to +y
        assert!(near(apply(&rotation_y(q), [1.0, 0.0, 0.0]), [0.0, 0.0, -1.0]));
        assert!(near(apply(&rotation_x(q), [0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]));
        assert!(near(apply(&rotation_z(q), [1.0, 0.0, 0.0]), [0.0, 1.0, 0.0]));
        // the Euler matrix is X by -x, then Y by -y, then Z by -z
        let a = [0.3, -0.7, 1.1];
        let composed = mul(&mul(&rotation_x(-a[0]), &rotation_y(-a[1])), &rotation_z(-a[2]));
        let e = euler(a);
        assert!(e.iter().zip(&composed).all(|(x, y)| (x - y).abs() < 1e-6), "{e:?} {composed:?}");
    }

    #[test]
    fn puppets_stand_where_their_update_puts_them() {
        use crate::layer::{Kind, Object};
        use crate::puppet::Puppet as Character;
        let mut fe = FrontEnd::new();
        let mut o = Object::new(Kind::Puppet, 0);
        o.pos = [1.0, 2.0, 3.0, 0.0];
        o.rot = [0.0, std::f32::consts::FRAC_PI_2, 0.0];
        fe.objects.push(o);
        // no character: none
        assert_eq!(fe.puppet_root(0), None);
        let ch = Character { character: Default::default(), callbacks: Default::default(), look_at: None };
        fe.puppets.insert(0, ch);
        // its angles (Y by -90 degrees: +x to +z) at its position
        let m = fe.puppet_root(0).unwrap();
        assert!(near(apply(&m, [1.0, 0.0, 0.0]), [1.0, 2.0, 4.0]));
        // flag 0x400000 turns it half round first: +x to -z
        fe.objects[0].flags |= 0x40_0000;
        assert!(near(apply(&fe.puppet_root(0).unwrap(), [1.0, 0.0, 0.0]), [1.0, 2.0, 2.0]));
        // on a spline: the spline's quaternion and position
        let set = |fe: &mut FrontEnd, off: u16, v: f32| fe.objects[0].set_slot(off, v.to_bits());
        fe.objects[0].set_slot(0x130, 1);
        for (off, v) in [(0xcc, 0.0), (0xd0, 0.0), (0xd4, 0.0), (0xd8, 1.0), (0xdc, 5.0), (0xe0, 6.0), (0xe4, 7.0)] {
            set(&mut fe, off, v);
        }
        assert!(near(apply(&fe.puppet_root(0).unwrap(), [1.0, 0.0, 0.0]), [6.0, 6.0, 7.0]));
        // a record the blend places (`+0x131`) at its quaternion and position; one it does not, none
        let region = crate::draw::Region { kind: 0, id: 0, x: 0, y: 0, w: 512, h: 512 };
        let r = Puppet {
            id: 1,
            object: 0,
            relative: true,
            pos: [9.0, 0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            region,
            stands: None,
            pose: None,
        };
        assert!(near(apply(&r.root().unwrap(), [0.0, 1.0, 0.0]), [9.0, 1.0, 0.0]));
        assert_eq!(Puppet { relative: false, ..r.clone() }.root(), None);
        // one it does not, where its character stands
        let stands = Some(placed(IDENTITY, [1.0, 2.0, 3.0]));
        assert_eq!(Puppet { relative: false, stands, ..r }.root(), stands);
    }

    #[test]
    fn models_turn_then_move() {
        let m = Model {
            id: 1,
            object: 0,
            resource: 0,
            pos: [10.0, 0.0, 5.0, 0.0],
            rot: [0.0, std::f32::consts::FRAC_PI_2, 0.0],
            extra: [0.0, 0.0, 0.0, 1.0],
            rotation_mode: 0,
            order: 0,
            field: 0,
            region: crate::draw::Region { kind: 0, id: 0, x: 0, y: 0, w: 512, h: 512 },
        };
        // turned about Y by -90 degrees (+x to +z), then moved
        assert!(near(apply(&m.world(), [1.0, 0.0, 0.0]), [10.0, 0.0, 6.0]));
        // a rotation mode takes the quaternion instead (the identity here)
        let q = Model { rotation_mode: 1, ..m };
        assert!(near(apply(&q.world(), [1.0, 0.0, 0.0]), [11.0, 0.0, 5.0]));
    }
}
