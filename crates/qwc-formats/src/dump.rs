//! JSON summaries of every format, used by the `qwc-dump` CLI and by the parity test against
//! `tools/dump_json.py`, which writes the same schema from the Python reference tools.
//! Large byte ranges are summarised by CRC-32; everything structural is written out in full.

use serde_json::{json, Map, Value};

use crate::util::{bad, crc32, hex, Result};
use crate::{
    audio, btf, ccd, cs_script, fas, ffn, flow, fpm, fsh, heightmap, mpf, pop, qsp, skl, sound_map, speech, sts,
    text_defs, vso, wpe,
};

/// File kinds the dumper understands, by extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ccd,
    Fsh,
    Skl,
    Fpm,
    Fas,
    Sfa,
    Btf,
    CsScript,
    Sts,
    Sdx,
    Ffn,
    Qsp,
    Vso,
    /// `frontend/ScrnLst/game.flo`
    Flo,
    /// `frontend/ScrnLst/Screens.lst`
    Lst,
    /// `.pot .lot .trl .got .cli .gro`
    TextDefs(text_defs::Ext),
    Fat,
    Mus,
    Hdr,
    /// `Audio/SpchCmn/<Game|FE>/Events.evt`
    Evt,
    /// `.map` sound-effect category map
    SoundMap,
    Mpf,
    /// `heightmap.csv`
    Heightmap,
    /// `match/<name>/matchdesc.txt`
    MatchDesc,
    Pop,
    Wpe,
    Tgq,
    /// `Audio/<lang>/dat.dat`; needs the repo layout around it (see [`dump_path`])
    SpeechDat,
    /// `Audio/banks.dat`; needs the repo layout around it (see [`dump_path`])
    BanksDat,
}

impl Kind {
    pub fn from_path(path: &str) -> Option<Kind> {
        if path.rsplit(['/', '\\']).next()?.eq_ignore_ascii_case("matchdesc.txt") {
            return Some(Kind::MatchDesc);
        }
        let ext = path.rsplit('.').next()?.to_ascii_lowercase();
        if let Some(e) = text_defs::Ext::from_ext(&ext) {
            return Some(Kind::TextDefs(e));
        }
        Some(match ext.as_str() {
            "ccd" => Kind::Ccd,
            "fsh" => Kind::Fsh,
            "skl" => Kind::Skl,
            "fpm" => Kind::Fpm,
            "fas" => Kind::Fas,
            "sfa" => Kind::Sfa,
            "btf" => Kind::Btf,
            "txt" => Kind::CsScript,
            "sts" => Kind::Sts,
            "sdx" => Kind::Sdx,
            "ffn" => Kind::Ffn,
            "qsp" => Kind::Qsp,
            "vso" => Kind::Vso,
            "flo" => Kind::Flo,
            "lst" => Kind::Lst,
            "fat" => Kind::Fat,
            "mus" => Kind::Mus,
            "hdr" => Kind::Hdr,
            "evt" => Kind::Evt,
            "map" => Kind::SoundMap,
            "mpf" => Kind::Mpf,
            "csv" => Kind::Heightmap,
            "pop" => Kind::Pop,
            "wpe" => Kind::Wpe,
            "tgq" => Kind::Tgq,
            "dat" => match path.rsplit(['/', '\\']).next()?.to_ascii_lowercase().as_str() {
                "dat.dat" => Kind::SpeechDat,
                "banks.dat" => Kind::BanksDat,
                _ => return None,
            },
            _ => return None,
        })
    }
}

/// Dump `data` as `kind`; `sfa` is the cutscene stream for `.fas` files that use Streamed3 channels.
pub fn dump(kind: Kind, data: &[u8], sfa: Option<&[u8]>) -> Result<Value> {
    Ok(match kind {
        Kind::Ccd => dump_ccd(data)?,
        Kind::Fsh => dump_fsh(data)?,
        Kind::Skl => dump_skl(data)?,
        Kind::Fpm => dump_fpm(data)?,
        Kind::Fas => dump_fas(data, sfa)?,
        Kind::Sfa => dump_sfa(data)?,
        Kind::Btf => dump_btf(data)?,
        Kind::CsScript => dump_cs(data),
        Kind::Sts => dump_sts(data)?,
        Kind::Sdx => dump_sdx(data)?,
        Kind::Ffn => dump_ffn(data)?,
        Kind::Qsp => dump_qsp(data)?,
        Kind::Vso => dump_vso(data)?,
        Kind::Flo => dump_flo(data)?,
        Kind::Lst => dump_lst(data)?,
        Kind::TextDefs(ext) => dump_text_defs(ext, data)?,
        Kind::Fat => dump_fat(data)?,
        Kind::Mus => json!({"format": "mus", "streams": streams(&audio::stream_run(data, 0, data.len(), 0x80)?)}),
        Kind::Hdr => dump_hdr(data)?,
        Kind::Evt => dump_evt(data)?,
        Kind::SoundMap => dump_map(data)?,
        Kind::Mpf => dump_mpf(data)?,
        Kind::Heightmap => json!({"format": "heightmap", "maps": heightmap::parse(data)?}),
        Kind::Pop => dump_pop(data)?,
        Kind::MatchDesc => {
            let records: Vec<Value> = crate::matchdesc::parse(data)?
                .into_iter()
                .map(|r| json!({"tag": r.tag, "fields": r.fields.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>()}))
                .collect();
            json!({"format": "matchdesc", "records": records})
        }
        Kind::Wpe => {
            let w = wpe::Wipe::parse(data)?;
            let crcs: Vec<u32> = w.frames.iter().map(|f| crc32(f)).collect();
            json!({"format": "wpe", "width": w.width, "height": w.height, "frame_crcs": crcs})
        }
        Kind::Tgq => dump_tgq(data)?,
        Kind::SpeechDat | Kind::BanksDat => return bad("dat.dat and banks.dat are dumped by path (dump_path)"),
    })
}

fn dump_ccd(data: &[u8]) -> Result<Value> {
    let c = ccd::Ccd::parse(data)?;
    let members: Vec<Value> = c
        .members
        .iter()
        .map(|m| json!({"name": m.name, "size": m.size, "size2": m.size2, "crc": crc32(c.member_data(m))}))
        .collect();
    Ok(json!({"format": "ccd", "version": c.version, "members": members}))
}

fn dump_fsh(data: &[u8]) -> Result<Value> {
    let f = fsh::Fsh::parse(data)?;
    let mut images = Vec::new();
    for (i, img) in f.images.iter().enumerate() {
        let b = img.blocks[0];
        let (_, _, rgba) = f.decode_rgba(i, 0)?;
        let pal: Vec<u8> = img.palette.iter().flatten().flatten().copied().collect();
        images.push(json!({
            "tag": img.tag, "type": b.kind, "w": b.w, "h": b.h, "cx": b.cx, "cy": b.cy, "x": b.x, "y": b.y,
            "blocks": img.blocks.iter().map(|b| json!([b.offset, b.kind, b.w])).collect::<Vec<_>>(),
            "level_sizes": img.level_sizes, "levels_fill_block": img.levels_fill_block,
            "palette_len": img.palette.as_ref().map(Vec::len), "palette_crc": img.palette.as_ref().map(|_| crc32(&pal)),
            "rgba_crc": crc32(&rgba),
        }));
    }
    Ok(json!({"format": "fsh", "magic": f.magic, "dir_id": f.dir_id, "images": images}))
}

fn dump_skl(data: &[u8]) -> Result<Value> {
    let s = skl::Skeleton::parse(data)?;
    let bones: Vec<Value> = s
        .bones
        .iter()
        .zip(&s.world)
        .map(|(b, w)| {
            json!({"index": b.index, "name": b.name, "flags": b.flags, "parent": b.parent, "child": b.child,
                   "sibling": b.sibling, "local": b.local, "world": w})
        })
        .collect();
    Ok(json!({"format": "skl", "bones": bones}))
}

fn dump_fpm(data: &[u8]) -> Result<Value> {
    let m = fpm::Fpm::parse(data)?;
    let mut groups = Vec::new();
    for g in &m.groups {
        let mut children = Vec::new();
        for c in &g.children {
            let mut draws = Vec::new();
            for dr in &c.draws {
                let ge = &dr.geom;
                let idx = m.indices(dr)?;
                let idx_bytes: Vec<u8> = idx.iter().flat_map(|i| i.to_le_bytes()).collect();
                let morphs = match m.morphs(dr)? {
                    None => Value::Null,
                    Some(mo) => {
                        let slots: Vec<Value> = mo
                            .slots
                            .iter()
                            .map(|s| match s {
                                None => Value::Null,
                                Some(d) => {
                                    d.iter().map(|(v, dv)| json!([v, dv[0], dv[1], dv[2]])).collect::<Vec<_>>().into()
                                }
                            })
                            .collect();
                        json!({"weight_base": mo.weight_base, "slots": slots})
                    }
                };
                draws.push(json!({
                    "entry": dr.entry, "bbox": dr.bbox, "opt1": dr.opt1, "opt2": dr.opt2, "tex": dr.tex,
                    "geom": {"off": ge.off, "vtx_off": ge.vtx_off, "idx_off": ge.idx_off, "fvf": ge.fvf, "h0": ge.h0,
                             "prim": ge.prim, "h1": ge.h1, "idx_count": ge.idx_count, "stride": ge.stride,
                             "ptype": ge.ptype, "vcount": ge.vcount,
                             "tail": ge.tail.iter().map(|b| format!("{b:02x}")).collect::<String>()},
                    "palette": m.palette(dr)?, "vertex_crc": crc32(m.vertex_bytes(dr)?), "index_crc": crc32(&idx_bytes),
                    "triangles": fpm::strip_to_tris(&idx).len(), "morphs": morphs,
                }));
            }
            children.push(json!({"entry": c.entry, "unk": c.unk, "draws": draws}));
        }
        groups.push(json!({"entry": g.entry, "children": children}));
    }
    let names: Vec<Value> = m.names.iter().map(|n| json!({"name": n.name, "hash": n.hash, "flags": n.flags})).collect();
    Ok(json!({"format": "fpm", "root_off": m.root_off, "geom_base": m.geom_base, "matrix": m.matrix,
              "names": names, "groups": groups}))
}

fn counted(c: Option<fas::CountedOff>) -> Value {
    c.map_or(Value::Null, |c| json!({"count": c.count, "off": c.off}))
}

fn dump_fas(data: &[u8], sfa: Option<&[u8]>) -> Result<Value> {
    let f = fas::Fas::parse(data)?;
    let sfa = sfa.map(fas::Sfa::parse).transpose()?;
    let mut anims = Vec::new();
    for a in &f.anims {
        let mut decoded = Vec::new();
        for slot in 0..2 {
            decoded.push(match f.channels(a, slot, sfa.as_ref()) {
                Ok(chs) => chs
                    .iter()
                    .map(|c| {
                        let ts = [0.0, a.frames as f64 / 3.0 + 0.5, a.frames as f64 - 0.25];
                        let samples: Vec<[f64; 3]> = ts.iter().map(|&t| c.sample(t, a.frames)).collect();
                        json!({"bone": c.bone, "times": c.times, "keys": c.keys, "samples": samples})
                    })
                    .collect::<Vec<_>>()
                    .into(),
                Err(e) if e.to_string().contains("needs the cutscene .sfa") => json!("needs_sfa"),
                Err(e) => return Err(e),
            });
        }
        anims.push(json!({
            "name": a.name, "unk0": a.unk0, "frames": a.frames, "fps": a.fps,
            "masks": a.masks.iter().map(|m| m.as_ref().map_or(Value::Null, |m| json!({"nbits": m.nbits, "mask": m.mask, "words": m.words}))).collect::<Vec<_>>(),
            "packed": a.packed, "channels": counted(Some(a.channels)), "root_off": a.root_off,
            "codecs": a.codecs.iter().map(|c| json!({"type": c.codec, "stride": c.stride, "count": c.count, "map_off": c.map_off, "off": c.off})).collect::<Vec<_>>(),
            "scales": a.scales.iter().map(|s| json!([s.0, s.1])).collect::<Vec<_>>(),
            "list4c": counted(a.list4c), "bbox": a.bbox, "list50": counted(a.list50), "decoded": decoded,
        }));
    }
    let h = f.header;
    Ok(json!({"format": "fas", "name": f.name, "flags": h.flags, "ver_major": h.ver_major, "version": h.version,
              "bulk_len": h.bulk_len, "tail": f.tail, "anims": anims}))
}

fn dump_sfa(data: &[u8]) -> Result<Value> {
    let s = fas::Sfa::parse(data)?;
    Ok(json!({"format": "sfa", "nframes": s.nframes, "data_off": s.data_off, "frame_size": s.frame_size,
              "channel_offs": s.channel_offs, "data_crc": crc32(&data[s.data_off as usize..])}))
}

fn dump_btf(data: &[u8]) -> Result<Value> {
    let b = btf::Btf::parse(data)?;
    let mut anims = Map::new();
    for (name, trig) in &b.anims {
        let t: Vec<Value> = trig
            .iter()
            .map(|t| {
                let mut v = json!({"u0": t.u0, "name": t.name, "times": t.times});
                if let Some(n) = &t.node {
                    v["node"] = n.clone().into();
                }
                v
            })
            .collect();
        // like a Python dict: a repeated name keeps the last value; a missing name is the key "null"
        anims.insert(name.clone().unwrap_or_else(|| "null".into()), t.into());
    }
    Ok(json!({"format": "btf", "anims": anims}))
}

fn dump_cs(data: &[u8]) -> Value {
    let blocks: Vec<Value> = cs_script::parse(data)
        .into_iter()
        .map(|b| {
            let e: Map<String, Value> = b.entries.into_iter().map(|(k, v)| (k, Value::String(v))).collect();
            json!({"kind": b.kind, "name": b.name, "entries": e})
        })
        .collect();
    json!({"format": "cs_script", "blocks": blocks})
}

fn dump_sts(data: &[u8]) -> Result<Value> {
    let s = sts::Sts::parse(data)?;
    let strings: Vec<Value> = s
        .entries
        .iter()
        .map(|e| {
            let ins: Vec<Value> =
                e.inserts.iter().map(|i| json!({"pos": i.pos, "type": i.ty, "id": i.id, "kind": i.kind})).collect();
            let units: Vec<u8> = e.units.iter().flat_map(|u| u.to_le_bytes()).collect();
            json!({"id": e.id, "inserts": ins, "text": sts::render(&e.units), "units_crc": crc32(&units)})
        })
        .collect();
    Ok(json!({"format": "sts", "language": s.language, "file_hash": s.file_hash, "unk_14": s.unk_14,
              "strings": strings}))
}

fn dump_sdx(data: &[u8]) -> Result<Value> {
    let s = sts::Sdx::parse(data)?;
    let strings: Vec<Value> = s
        .strings
        .iter()
        .map(|e| json!({"id": e.id, "file_hash": e.file_hash, "unk": e.unk, "index": e.index}))
        .collect();
    let files: Vec<Value> = s.files.iter().map(|(h, n)| json!({"hash": h, "name": n})).collect();
    Ok(json!({"format": "sdx", "strings": strings, "files": files}))
}

fn dump_ffn(data: &[u8]) -> Result<Value> {
    let f = ffn::Ffn::parse(data)?;
    let glyphs: Vec<Value> = f
        .glyphs
        .iter()
        .map(|g| {
            json!({"code": g.code, "w": g.w, "h": g.h, "u": g.u, "v": g.v, "x_offset": g.x_offset,
                   "y_offset": g.y_offset, "advance": g.advance, "n_kern": g.n_kern, "kern_first": g.kern_first})
        })
        .collect();
    let kerning: Vec<Value> =
        f.kerning.iter().map(|k| json!({"prev": k.prev, "next": k.next, "adjust": k.adjust})).collect();
    Ok(json!({
        "format": "ffn", "unk_08": f.unk_08, "flags": f.flags, "line_a": f.line_a, "line_b": f.line_b,
        "kern_off": f.kern_off, "tex_off": f.tex_off, "desc_off": f.desc_off, "glyphs": glyphs, "kerning": kerning,
        "header_crc": crc32(&data[..ffn::GLYPH_TABLE]), "padding_len": f.padding.len(), "padding_crc": crc32(f.padding),
        "descriptor_crc": crc32(f.descriptor), "texture": [f.tex_w, f.tex_h],
        "blocks": f.blocks.iter().map(|b| json!([b.offset, b.kind, b.w])).collect::<Vec<_>>(),
        "texture_crc": crc32(&data[f.tex_off as usize..]), "rgba_crc": crc32(&f.atlas()?),
    }))
}

fn dump_qsp(data: &[u8]) -> Result<Value> {
    let q = qsp::Qsp::parse(data)?;
    let splines: Vec<Value> = q
        .splines
        .iter()
        .map(|s| {
            let pts: Vec<Value> =
                s.points.iter().map(|p| json!({"pos": p.pos, "flags": p.flags, "rot": p.rot})).collect();
            json!({"name": s.name, "points": pts})
        })
        .collect();
    Ok(json!({"format": "qsp", "unk_0c": q.unk_0c, "splines": splines}))
}

fn layer_json(l: &vso::Layer) -> Value {
    match l {
        vso::Layer::Object { kind, fields, id, flags, children } => json!({
            "type": kind, "fields": fields, "id": id, "flags": flags,
            "children": children.iter().map(layer_json).collect::<Vec<_>>(),
        }),
        vso::Layer::Property { name, words } => json!({"property": name, "words": words}),
    }
}

fn dump_vso(data: &[u8]) -> Result<Value> {
    let v = vso::Vso::parse(data)?;
    let fns = v.functions()?;
    let mut instructions = 0;
    for f in &fns {
        instructions += v.disassemble(f.start, f.end)?.len();
    }
    let (roots, note) = v.layers()?;
    let symbols: Vec<Value> = v
        .symbols
        .iter()
        .map(|s| json!({"name": s.name, "hash": s.hash, "section": s.section, "value": s.value, "flags": s.flags}))
        .collect();
    let resources: Vec<Value> = v
        .resources()?
        .iter()
        .map(|r| {
            let mut j = json!({"id": r.id, "group": r.group, "name": r.name, "type": r.kind});
            if let Some(c) = &r.choices {
                let entries: Vec<Value> =
                    c.entries.iter().map(|(g, n, k)| json!({"group": g, "name": n, "type": k})).collect();
                j["choices"] = json!({"function": c.function, "entries": entries});
            }
            j
        })
        .collect();
    Ok(json!({
        "format": "vso", "version": v.version, "hash": v.hash, "flags": v.flags,
        "sizes": {"code": v.code.len(), "data": v.data.len(), "lit": v.lit.len(), "bss": v.bss_size},
        "symbols": symbols,
        "imports": v.imports.iter().map(|i| json!({"name": i.name, "hash": i.hash})).collect::<Vec<_>>(),
        "relocs": v.relocs.iter().map(|r| json!({"section": r.section, "offset": r.offset, "target": r.target, "addend": r.addend})).collect::<Vec<_>>(),
        "irefs": v.irefs.iter().map(|r| json!({"offset": r.offset, "import": r.import})).collect::<Vec<_>>(),
        "functions": fns.iter().map(|f| json!({"name": f.name, "start": f.start, "end": f.end})).collect::<Vec<_>>(),
        "instructions": instructions, "listing_crc": crc32(v.listing()?.as_bytes()),
        "layers": roots.iter().map(layer_json).collect::<Vec<_>>(), "layer_quirk": note, "resources": resources,
    }))
}

fn dump_flo(data: &[u8]) -> Result<Value> {
    let f = flow::Flow::parse(data)?;
    let groups: Vec<Value> = f
        .groups
        .iter()
        .map(|g| json!({"hash": g.hash, "entry": g.entry, "n_children": g.n_children, "name": g.name, "parent": g.parent}))
        .collect();
    let nodes: Vec<Value> = f
        .nodes
        .iter()
        .map(|n| {
            json!({"hash": n.hash, "str_hashes": n.str_hashes, "id": n.id, "first_link": n.first_link,
                   "n_links": n.n_links, "name": n.name, "strings": n.strings, "group": n.group})
        })
        .collect();
    Ok(json!({
        "format": "flo", "version": f.version, "has_names": f.has_names, "roots": f.roots, "groups": groups,
        "nodes": nodes, "links": f.links.iter().map(|l| json!({"target": l.target, "flags": l.flags})).collect::<Vec<_>>(),
    }))
}

fn dump_lst(data: &[u8]) -> Result<Value> {
    let named =
        |v: &[(String, u32)], key: &str| -> Vec<Value> { v.iter().map(|(n, h)| json!({key: n, "hash": h})).collect() };
    let screens: Vec<Value> = flow::parse_screens(data)?
        .iter()
        .map(|s| {
            json!({"name": s.name, "hash": s.hash, "loadsets": named(&s.loadsets, "name"),
                   "dynamic_loadsets": named(&s.dynamic_loadsets, "key"),
                   "layers": s.layers.iter().map(|l| json!({"priority": l.priority, "group": l.group, "file": l.file, "reload": l.reload})).collect::<Vec<_>>(),
                   "ignored": s.ignored})
        })
        .collect();
    Ok(json!({"format": "lst", "screens": screens}))
}

fn dump_text_defs(ext: text_defs::Ext, data: &[u8]) -> Result<Value> {
    use text_defs::{FxValue, Parsed};
    let p = text_defs::parse(ext, data)?;
    let problems = text_defs::vocab_problems(ext, &p);
    Ok(match p {
        Parsed::Fx { blocks, leftover } => {
            let blocks: Vec<Value> = blocks
                .iter()
                .map(|b| {
                    let values: Vec<Value> = b
                        .values
                        .iter()
                        .map(|(k, v)| match v {
                            FxValue::One(s) => json!([k, s]),
                            FxValue::Many(l) => json!([k, l]),
                        })
                        .collect();
                    json!({"kind": b.kind, "name": b.name, "values": values, "unparsed": b.unparsed})
                })
                .collect();
            json!({"format": "text_defs", "kind": "fx", "blocks": blocks, "leftover": leftover, "problems": problems})
        }
        Parsed::Cli { values, unparsed } => {
            json!({"format": "text_defs", "kind": "cli", "values": values, "unparsed": unparsed, "problems": problems})
        }
        Parsed::Gro { materials } => {
            json!({"format": "text_defs", "kind": "gro", "materials": materials, "problems": problems})
        }
    })
}

fn streams(s: &[audio::Stream]) -> Vec<Value> {
    s.iter()
        .map(|s| {
            json!({"offset": s.offset, "size": s.size, "blocks": s.blocks, "channels": s.channels, "rate": s.rate,
                   "samples": s.samples, "codec": s.codec})
        })
        .collect()
}

fn dump_fat(data: &[u8]) -> Result<Value> {
    let f = audio::Fat::parse(data)?;
    let nodes: Vec<Value> = f
        .nodes
        .iter()
        .map(|n| json!({"hash": n.hash, "first": n.first, "count": n.count, "subdirs": n.subdirs}))
        .collect();
    let files: Vec<Value> =
        f.files.iter().map(|f| json!({"hash": f.hash, "offset": f.offset, "size": f.size})).collect();
    Ok(json!({"format": "fat", "nodes": nodes, "files": files}))
}

fn dump_hdr(data: &[u8]) -> Result<Value> {
    let (path, off, sentence) = audio::read_hdr(data)?;
    let b = speech::parse_bank(sentence)?;
    let clips: Vec<Value> = b.clips.iter().map(|c| json!({"offset": c.offset, "keys": c.keys})).collect();
    let bank = json!({"id": b.id, "team": b.team, "keys": b.keys, "history": b.history, "size": b.size,
                      "clips": clips, "key_masks": b.key_masks, "parts": b.parts});
    Ok(json!({"format": "hdr", "path": path, "data_off": off, "sentence_len": sentence.len(),
              "sentence_crc": crc32(sentence), "bank": bank}))
}

fn dump_evt(data: &[u8]) -> Result<Value> {
    let ev = speech::parse_evt(data)?;
    let events: Vec<Value> = ev
        .events
        .iter()
        .map(|e| {
            let variants: Vec<Value> = e
                .variants
                .iter()
                .map(|v| {
                    let parts: Vec<Value> = v
                        .parts
                        .iter()
                        .map(|p| {
                            let keys: Vec<Value> = p
                                .keys
                                .iter()
                                .map(|k| {
                                    json!({"allowed": k.allowed, "source": k.source, "x5": k.x5,
                                                "ref": k.r#ref, "x7": k.x7})
                                })
                                .collect();
                            json!({"bank": p.bank, "arg": p.arg, "bank_mode": p.bank_mode, "keys": keys})
                        })
                        .collect();
                    json!({"weight": v.weight, "chance": v.chance, "test_mask": v.test_mask,
                           "test_value": v.test_value, "mode": v.mode, "slots": v.slots, "masks": v.masks,
                           "parts": parts})
                })
                .collect();
            let mut o = json!({"id": e.id, "t0": e.t0, "t1": e.t1, "max_repeats": e.max_repeats,
                               "chance": e.chance, "flags": e.flags, "args": e.args, "tests": e.tests,
                               "arg_tests": e.arg_tests, "variants": variants, "name": e.name});
            if let (Some(m), Some(g)) = (e.arg_modes, &e.groups) {
                o["arg_modes"] = json!(m);
                o["groups"] = json!(g);
            }
            if let Some(l) = &e.linked {
                o["linked"] = json!(l);
            }
            o
        })
        .collect();
    Ok(json!({"format": "evt", "set": ev.set, "unknown": ev.unknown, "defaults": ev.defaults, "events": events}))
}

fn dump_map(data: &[u8]) -> Result<Value> {
    let m = sound_map::SoundMap::parse(data)?;
    let banks: Vec<Value> = m
        .banks
        .iter()
        .map(|b| json!({"name": b.name, "flags": b.flags, "hash": sound_map::bank_file_hash(&b.name)}))
        .collect();
    let groups: Vec<Value> = m
        .groups
        .iter()
        .map(|g| {
            let events: Vec<Value> = g
                .events
                .iter()
                .map(|e| {
                    let variants: Vec<Value> = e
                        .variants
                        .iter()
                        .map(|v| {
                            let samples: Vec<Value> = v
                                .samples
                                .iter()
                                .map(|s| {
                                    json!({"sound": s.sound, "weight": s.weight, "unk_08": s.unk_08, "bank": s.bank,
                                           "unk_0e": s.unk_0e})
                                })
                                .collect();
                            let links: Vec<Value> =
                                v.links.iter().map(|&(variant, value)| json!({"variant": variant, "value": value})).collect();
                            json!({"samples": samples, "links": links, "fields": hex(&v.fields)})
                        })
                        .collect();
                    json!({"event_id": e.event_id, "id": e.id, "value": e.value, "flags": e.flags, "variants": variants})
                })
                .collect();
            json!({"id": g.id, "flags": g.flags, "scale": g.scale, "unk": g.unk, "events": events})
        })
        .collect();
    Ok(json!({"format": "map", "stamp": m.stamp, "banks": banks, "groups": groups}))
}

fn dump_mpf(data: &[u8]) -> Result<Value> {
    let m = mpf::MusicFile::parse(data)?;
    let nodes: Vec<Value> = m
        .nodes
        .iter()
        .map(|n| {
            let branches: Vec<Value> =
                n.branches.iter().map(|b| json!({"a": b.a, "b": b.b, "target": b.target})).collect();
            json!({"offset": n.offset, "sample": n.sample, "fields": hex(&n.fields), "branches": branches})
        })
        .collect();
    let events: Vec<Value> = m
        .events
        .iter()
        .map(|e| {
            let actions: Vec<String> = e.actions.iter().map(|a| hex(a)).collect();
            json!({"offset": e.offset, "head": hex(&e.head), "actions": actions})
        })
        .collect();
    let routers: Vec<String> = m.routers.iter().map(|r| hex(r)).collect();
    let tracks: Vec<Vec<Value>> = m
        .tracks
        .iter()
        .map(|t| t.iter().map(|s| json!({"offset": s.offset, "duration_ms": s.duration_ms})).collect())
        .collect();
    Ok(json!({"format": "mpf", "n_sections": m.n_sections, "n_vars": m.n_vars, "nodes": nodes, "events": events,
              "routers": routers, "tracks": tracks}))
}

fn dump_pop(data: &[u8]) -> Result<Value> {
    let locators: Vec<Value> = pop::parse(data)?.iter().map(|l| json!({"name": l.name, "matrix": l.matrix})).collect();
    Ok(json!({"format": "pop", "locators": locators}))
}

/// `ROOT/original/cd_files/Audio/<lang>/dat.dat` or `ROOT/original/cd_files/Audio/banks.dat`, indexed with the
/// `.fat` and `.hdr` files under `ROOT/extracted/Audio`.
fn dump_audio_dat(kind: Kind, path: &std::path::Path) -> Result<Value> {
    let path = std::path::absolute(path).map_err(|e| crate::Error::Format(format!("{}: {e}", path.display())))?;
    let up = |n: usize| path.ancestors().nth(n).ok_or_else(|| crate::Error::Format("path too short".into()));
    if kind == Kind::BanksDat {
        let banks: Vec<Value> = audio::bank_index_at(up(4)?)?
            .into_iter()
            .map(|(hash, b)| match b {
                audio::Bank::Bnkl(n) => json!({"hash": hash, "kind": "BNKl", "sounds": n}),
                audio::Bank::Stream(s) => json!({"hash": hash, "kind": "stream", "streams": streams(&s)}),
            })
            .collect();
        return Ok(json!({"format": "banks", "banks": banks}));
    }
    let lang = up(1)?.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let files: Vec<Value> = audio::speech_index_at(up(5)?, &lang)?
        .into_iter()
        .map(|(name, s)| json!({"name": name, "streams": streams(&s)}))
        .collect();
    Ok(json!({"format": "speech", "lang": lang, "files": files}))
}

/// Dump a file by path (kind from the extension).
pub fn dump_path(path: &str, sfa: Option<&str>) -> Result<Value> {
    let Some(kind) = Kind::from_path(path) else {
        return bad(format!("unknown file kind: {path}"));
    };
    if matches!(kind, Kind::SpeechDat | Kind::BanksDat) {
        return dump_audio_dat(kind, std::path::Path::new(path));
    }
    let data = std::fs::read(path).map_err(|e| crate::Error::Format(format!("{path}: {e}")))?;
    let sfa = sfa.map(std::fs::read).transpose().map_err(|e| crate::Error::Format(format!("sfa: {e}")))?;
    dump(kind, &data, sfa.as_deref())
}

/// A movie: its size, each picture's quantiser and payload CRC, and the bits the first three pictures take.
fn dump_tgq(data: &[u8]) -> Result<Value> {
    let m = crate::tqi::Movie::parse(data)?;
    let crcs: Vec<u32> = (0..m.frames.len()).map(|k| crc32(m.payload(k))).collect();
    let bits = (0..m.frames.len().min(3)).map(|k| m.frame_and_bits(k).map(|(_, b)| b)).collect::<Result<Vec<_>>>()?;
    Ok(json!({"format": "tgq", "width": m.width, "height": m.height, "quantisers": m.quantisers(),
              "payload_crcs": crcs, "bits": bits}))
}
