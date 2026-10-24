//! Skeletons (`.skl`, magic "skl1"; port of `tools/skl_parse.py`, loader check at `0x408b3a`).
//!
//! ```text
//! "skl1" | u32 bone_count | u32 bones_off (0x10) | u32 ?
//! bone (0x58): u32 index | u32 parent_off | u32 first_child_off | u32 next_sibling_off  (file-relative, 0 = none)
//!              f32[16] local matrix (row-vector D3D layout, translation in [12..14], Y up)
//!              u32 name_off | u32 flags (nonzero only on *Mesh attachment bones)
//! ```

use crate::util::{bad, cstr_at, f32s, slice, u32_at, Result};

pub const BONE_SIZE: usize = 0x58;

#[derive(Debug, Clone)]
pub struct Bone {
    pub index: u32,
    pub name: String,
    pub local: [f32; 16],
    pub flags: u32,
    pub parent: Option<usize>,
    pub child: Option<usize>,
    pub sibling: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
    /// Bind-pose world matrices (`local * parent_world`, row-vector convention), in f64 like the Python tool.
    pub world: Vec<[f64; 16]>,
}

/// Row-major 4x4 multiply (row-vector convention: `v * a * b`).
pub fn matmul(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    let mut out = [0f64; 16];
    for r in 0..4 {
        for c in 0..4 {
            out[r * 4 + c] = (0..4).map(|k| a[r * 4 + k] * b[k * 4 + c]).sum();
        }
    }
    out
}

pub fn transform(m: &[f64; 16], p: [f64; 3]) -> [f64; 3] {
    let [x, y, z] = p;
    [
        x * m[0] + y * m[4] + z * m[8] + m[12],
        x * m[1] + y * m[5] + z * m[9] + m[13],
        x * m[2] + y * m[6] + z * m[10] + m[14],
    ]
}

pub fn transform_dir(m: &[f64; 16], n: [f64; 3]) -> [f64; 3] {
    let [x, y, z] = n;
    [x * m[0] + y * m[4] + z * m[8], x * m[1] + y * m[5] + z * m[9], x * m[2] + y * m[6] + z * m[10]]
}

impl Skeleton {
    pub fn parse(d: &[u8]) -> Result<Skeleton> {
        if slice(d, 0, 4)? != b"skl1" {
            return bad("not skl1");
        }
        let count = u32_at(d, 4)? as usize;
        let base = u32_at(d, 8)? as usize;
        let bone_index = |off: u32| -> Result<Option<usize>> {
            if off == 0 {
                return Ok(None);
            }
            let rel = (off as usize).checked_sub(base).filter(|r| r % BONE_SIZE == 0 && r / BONE_SIZE < count);
            rel.map(|r| Some(r / BONE_SIZE)).ok_or_else(|| crate::Error::Format(format!("bad bone link {off:#x}")))
        };
        let mut bones = Vec::with_capacity(count);
        for i in 0..count {
            let r = base + i * BONE_SIZE;
            bones.push(Bone {
                index: u32_at(d, r)?,
                parent: bone_index(u32_at(d, r + 4)?)?,
                child: bone_index(u32_at(d, r + 8)?)?,
                sibling: bone_index(u32_at(d, r + 12)?)?,
                local: f32s::<16>(d, r + 0x10)?,
                name: cstr_at(d, u32_at(d, r + 0x50)? as usize)?,
                flags: u32_at(d, r + 0x54)?,
            });
        }
        let mut world: Vec<Option<[f64; 16]>> = vec![None; count];
        for i in 0..count {
            Self::world_of(&bones, &mut world, i, 0)?;
        }
        Ok(Skeleton { bones, world: world.into_iter().map(Option::unwrap).collect() })
    }

    /// Where each node's morph weights start in a character's weight array: the nodes' morph counts summed in node
    /// order. **[I]** the count is the node's `flags` (4 on `TailMesh` and the hands' `*HandMesh`, 19 on `HeadMesh`, 7
    /// on `HairMesh`, 0 on every other node of the 22 character skeletons); the models' draws start their morph
    /// slots at exactly these sums (`fpm` `weight_base`, all 1,830 draws).
    pub fn morph_bases(&self) -> Vec<u16> {
        let mut at = 0u16;
        self.bones
            .iter()
            .map(|b| {
                let base = at;
                at = at.saturating_add(b.flags.min(0xff) as u16);
                base
            })
            .collect()
    }

    fn world_of(bones: &[Bone], world: &mut [Option<[f64; 16]>], i: usize, depth: usize) -> Result<[f64; 16]> {
        if let Some(w) = world[i] {
            return Ok(w);
        }
        if depth > bones.len() {
            return bad("cycle in bone hierarchy");
        }
        let local = bones[i].local.map(f64::from);
        let w = match bones[i].parent {
            None => local,
            Some(p) => matmul(&local, &Self::world_of(bones, world, p, depth + 1)?),
        };
        world[i] = Some(w);
        Ok(w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_bone_chain() {
        let mut d = b"skl1".to_vec();
        for v in [2u32, 0x10, 0] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        let names_off = 0x10 + 2 * BONE_SIZE;
        for (i, (parent, ty)) in [(0u32, 1.0f32), (0x10, 2.0)].into_iter().enumerate() {
            for v in [i as u32, parent, 0, 0] {
                d.extend_from_slice(&v.to_le_bytes());
            }
            let mut m = [0f32; 16];
            m[0] = 1.0;
            m[5] = 1.0;
            m[10] = 1.0;
            m[15] = 1.0;
            m[13] = ty;
            for v in m {
                d.extend_from_slice(&v.to_le_bytes());
            }
            d.extend_from_slice(&((names_off + 2 * i) as u32).to_le_bytes());
            d.extend_from_slice(&0u32.to_le_bytes());
        }
        d.extend_from_slice(b"A\0B\0");
        let s = Skeleton::parse(&d).unwrap();
        assert_eq!(s.bones[1].name, "B");
        assert_eq!(s.bones[1].parent, Some(0));
        assert_eq!(s.world[1][13], 3.0);
        assert_eq!(transform(&s.world[1], [1.0, 0.0, 0.0]), [1.0, 3.0, 0.0]);
    }

    #[test]
    fn morph_bases_sum_the_nodes_counts_in_order() {
        let bone = |name: &str, flags| Bone {
            index: 0,
            name: name.into(),
            local: [0.0; 16],
            flags,
            parent: None,
            child: None,
            sibling: None,
        };
        let s = Skeleton {
            bones: vec![bone("Root", 0), bone("TailMesh", 4), bone("HeadMesh", 19), bone("RightHandMesh", 4)],
            world: vec![[0.0; 16]; 4],
        };
        assert_eq!(s.morph_bases(), [0, 0, 4, 23]);
    }
}
