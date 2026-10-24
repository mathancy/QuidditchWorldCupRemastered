//! Normals for meshes that carry none (the stadiums': the original draws them unlit, so they need none), for the
//! remastered lighting ([`crate::Look::Remastered`]). Not the original's: the remaster's own.
//!
//! Each vertex takes the area-weighted normals of the triangles around its position (vertices at the same place
//! are one, so a texture seam stays smooth), but only the triangles within [`CREASE`] of its own triangles' mean:
//! a wall's corner stays sharp, a curved bank smooth. A triangle's front is its winding's as the meshes store it
//! (counter-clockwise, `qwc_assets::model::Mesh::indices`).

use std::collections::HashMap;

use glam::Vec3;

/// The widest angle between triangles that are shaded as one surface.
pub const CREASE: f32 = 50.0;

/// One normal per vertex of the triangle list `indices` over `positions`; a vertex no triangle uses gets zero.
pub fn smooth(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    if indices.iter().any(|&i| i as usize >= positions.len()) {
        return vec![[0.0; 3]; positions.len()];
    }
    let p = |i: u32| Vec3::from(positions[i as usize]);
    // each triangle's normal, its length twice its area
    let faces: Vec<Vec3> = indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let n = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
            if n.is_finite() {
                n
            } else {
                Vec3::ZERO
            }
        })
        .collect();
    // the triangles at each place, and each vertex's own
    let key = |v: [f32; 3]| v.map(f32::to_bits);
    let mut at: HashMap<[u32; 3], Vec<usize>> = HashMap::new();
    let mut own: Vec<Vec<usize>> = vec![Vec::new(); positions.len()];
    for (f, t) in indices.as_chunks::<3>().0.iter().enumerate() {
        for &i in t {
            own[i as usize].push(f);
            let list = at.entry(key(positions[i as usize])).or_default();
            if list.last() != Some(&f) {
                list.push(f);
            }
        }
    }
    let cos = CREASE.to_radians().cos();
    (0..positions.len())
        .map(|v| {
            let mine = own[v].iter().map(|&f| faces[f]).sum::<Vec3>().normalize_or_zero();
            if mine == Vec3::ZERO {
                return [0.0; 3];
            }
            let around = &at[&key(positions[v])];
            let n: Vec3 = around.iter().map(|&f| faces[f]).filter(|f| f.normalize_or_zero().dot(mine) >= cos).sum();
            n.normalize_or(mine).to_array()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_quad_faces_its_front() {
        // two triangles in the y = 0 plane, wound as the meshes store their fronts
        let pos = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 1.0]];
        let n = smooth(&pos, &[0, 2, 1, 1, 2, 3]);
        let up = n.iter().map(|n| Vec3::from(*n)).collect::<Vec<_>>();
        assert!(up.iter().all(|n| (n.y.abs() - 1.0).abs() < 1e-6), "{up:?}");
        // the other winding faces the other way
        let m = smooth(&pos, &[0, 1, 2, 1, 3, 2]);
        assert!((Vec3::from(m[0]) + up[0]).length() < 1e-6);
    }

    #[test]
    fn corners_stay_sharp_and_seams_smooth() {
        // a floor and a wall meeting at x = 1, sharing the edge's places but not its vertices
        let pos = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
        ];
        let n = smooth(&pos, &[0, 2, 1, 1, 2, 3, 4, 6, 5, 5, 6, 7]);
        // the floor's vertices at the corner keep the floor's normal, the wall's the wall's
        assert!(Vec3::from(n[1]).abs().abs_diff_eq(Vec3::Y, 1e-6), "{:?}", n[1]);
        assert!(Vec3::from(n[4]).abs().abs_diff_eq(Vec3::X, 1e-6), "{:?}", n[4]);
        // a gentle fold (10 degrees) is smoothed across: its shared vertices lean between the two
        let t = 10f32.to_radians();
        let pos = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 1.0], [1.0 + t.cos(), t.sin(), 0.0]];
        let n = smooth(&pos, &[0, 2, 1, 1, 2, 3, 1, 3, 4]);
        let lean = Vec3::from(n[1]);
        assert!(lean.x.abs() > 0.01 && lean.y.abs() > 0.9, "{lean:?}");
    }
}
