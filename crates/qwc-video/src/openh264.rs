//! OpenH264 (built from its source) for the pictures, Symphonia for the MP4 and the AAC: the decoder for every
//! system. OpenH264 decodes Constrained Baseline fully (the remastered movies are converted to it: no B-frames), so
//! a picture comes out for each sample in order.

use std::fs::File;

use ::openh264::decoder::Decoder as H264;
use ::openh264::formats::YUVSource;
use symphonia::core::codecs::audio::well_known::CODEC_ID_AAC;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::video::well_known::extra_data::VIDEO_EXTRA_DATA_ID_AVC_DECODER_CONFIG;
use symphonia::core::codecs::video::well_known::CODEC_ID_H264;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use super::*;
use crate::yuv::{shrunk, to_bgra, Planes};

fn reader(path: &Path) -> Result<Box<dyn FormatReader>, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("mp4");
    symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| e.to_string())
}

/// The H.264 track.
struct Video {
    track: u32,
    /// Seconds per tick of its time stamps.
    tick: f64,
    size: (u32, u32),
    /// The bytes of each NAL unit's length in a sample.
    length: usize,
    /// The sequence and picture parameter sets, as an Annex B stream.
    params: Vec<u8>,
}

/// The `AVCDecoderConfigurationRecord`: the NAL length's size, and the parameter sets as Annex B.
fn avc_config(d: &[u8]) -> Option<(usize, Vec<u8>)> {
    let length = (*d.get(4)? & 3) as usize + 1;
    let mut params = Vec::new();
    let mut i = 5;
    // the sequence sets (a 5-bit count), then the picture sets (8-bit)
    for mask in [0x1f, 0xff] {
        let n = (*d.get(i)? & mask) as usize;
        i += 1;
        for _ in 0..n {
            let len = u16::from_be_bytes([*d.get(i)?, *d.get(i + 1)?]) as usize;
            params.extend([0, 0, 0, 1]);
            params.extend(d.get(i + 2..i + 2 + len)?);
            i += 2 + len;
        }
    }
    Some((length, params))
}

/// A sample's NAL units (each after a `length`-byte size) as Annex B, into `out`.
fn annex_b(mut data: &[u8], length: usize, out: &mut Vec<u8>) {
    out.clear();
    while data.len() >= length {
        let n = data[..length].iter().fold(0usize, |a, &b| a << 8 | b as usize);
        let Some(nal) = data.get(length..length + n) else { break };
        out.extend([0, 0, 0, 1]);
        out.extend(nal);
        data = &data[length + n..];
    }
}

fn video(r: &dyn FormatReader) -> Result<Video, String> {
    let track = r
        .tracks()
        .iter()
        .find(|t| matches!(&t.codec_params, Some(CodecParameters::Video(v)) if v.codec == CODEC_ID_H264))
        .ok_or("no H.264 track")?;
    let Some(CodecParameters::Video(v)) = &track.codec_params else { unreachable!() };
    let config = v.extra_data.iter().find(|x| x.id == VIDEO_EXTRA_DATA_ID_AVC_DECODER_CONFIG).ok_or("no avcC")?;
    let (length, params) = avc_config(&config.data).ok_or("bad avcC")?;
    let tb = track.time_base.ok_or("no time base")?;
    Ok(Video {
        track: track.id,
        tick: tb.numer.get() as f64 / tb.denom.get() as f64,
        size: (v.width.ok_or("no width")? as u32, v.height.ok_or("no height")? as u32),
        length,
        params,
    })
}

/// The AAC track, whole, as 16-bit PCM.
fn sound(path: &Path) -> Result<Pcm, String> {
    let mut r = reader(path)?;
    let (id, params) = r
        .tracks()
        .iter()
        .find_map(|t| match &t.codec_params {
            Some(CodecParameters::Audio(a)) if a.codec == CODEC_ID_AAC => Some((t.id, a.clone())),
            _ => None,
        })
        .ok_or("no AAC track")?;
    let mut dec = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| e.to_string())?;
    let mut rate = params.sample_rate.unwrap_or(48000);
    let mut channels = params.channels.as_ref().map_or(2, |c| c.count());
    let (mut samples, mut block) = (Vec::new(), Vec::<i16>::new());
    while let Some(p) = r.next_packet().map_err(|e| e.to_string())? {
        if p.track_id != id {
            continue;
        }
        match dec.decode(&p) {
            Ok(buf) => {
                rate = buf.spec().rate();
                channels = buf.spec().channels().count();
                buf.copy_to_vec_interleaved(&mut block);
                samples.extend_from_slice(&block);
            }
            // a damaged packet is skipped
            Err(symphonia::core::errors::Error::DecodeError(_)) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(Pcm { channels, rate, samples })
}

/// The pictures' thread.
fn pictures(path: PathBuf, v: Video, shrink: u32, tx: SyncSender<Result<Picture, String>>) {
    let run = || -> Result<(), String> {
        let mut r = reader(&path)?;
        let mut dec = H264::new().map_err(|e| e.to_string())?;
        dec.decode(&v.params).map_err(|e| e.to_string())?;
        let mut nal = Vec::new();
        while let Some(p) = r.next_packet().map_err(|e| e.to_string())? {
            if p.track_id != v.track {
                continue;
            }
            annex_b(&p.data, v.length, &mut nal);
            let Some(yuv) = dec.decode(&nal).map_err(|e| e.to_string())? else { continue };
            let (y_stride, c_stride, _) = yuv.strides();
            let planes = Planes { y: yuv.y(), u: yuv.u(), v: yuv.v(), y_stride, c_stride, size: yuv.dimensions() };
            let at = p.pts.get() as f64 * v.tick;
            if tx.send(Ok(Picture { at, bgra: to_bgra(&planes, shrink) })).is_err() {
                break;
            }
        }
        Ok(())
    };
    if let Err(e) = run() {
        tx.send(Err(format!("{}: {e}", path.display()))).ok();
    }
}

/// Open `path`: its sound decoded, its pictures starting on their thread, halved while they stay at least as large
/// as they are shown within `within`.
pub fn open(path: &Path, within: (u32, u32)) -> Result<HdMovie, String> {
    let v = video(&*reader(path)?)?;
    let want = fitted(v.size, within);
    let mut shrink = 0;
    while shrink < 4 && v.size.0 >> (shrink + 1) >= want.0 && v.size.1 >> (shrink + 1) >= want.1 {
        shrink += 1;
    }
    let (w, h) = shrunk((v.size.0 as usize, v.size.1 as usize), shrink);
    let sound = match sound(path) {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("{}: sound: {e}", path.display());
            None
        }
    };
    let (size, owned) = (v.size, path.to_path_buf());
    HdMovie::start(size, (w as u32, h as u32), sound, "OpenH264", move |tx| pictures(owned, v, shrink, tx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_and_samples() {
        // version 1, profile 66, compat 0xc0, level 50, 4-byte lengths; one SPS [0x67, 1], one PPS [0x68]
        let avcc = [1, 66, 0xc0, 50, 0xff, 0xe1, 0, 2, 0x67, 1, 1, 0, 1, 0x68];
        let (length, params) = avc_config(&avcc).unwrap();
        assert_eq!(length, 4);
        assert_eq!(params, [0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68]);
        assert_eq!(avc_config(&avcc[..10]), None);
        let mut out = Vec::new();
        annex_b(&[0, 0, 0, 2, 0x65, 9, 0, 0, 0, 1, 0x06], 4, &mut out);
        assert_eq!(out, [0, 0, 0, 1, 0x65, 9, 0, 0, 0, 1, 0x06]);
    }
}
