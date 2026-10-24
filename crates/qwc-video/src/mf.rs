//! Media Foundation's source reader (Windows): the pictures decoded, converted to 32-bit RGB and scaled by the
//! reader (its advanced video processing), the sound converted to 16-bit PCM.

use super::*;
use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

fn startup() -> Result<(), String> {
    static STARTED: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    STARTED.get_or_init(|| unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL).map_err(|e| e.to_string()) }).clone()
}

/// A source reader on `path` with only `stream` selected, giving `subtype` (pictures at `size` when given).
unsafe fn reader(
    path: &Path,
    stream: MF_SOURCE_READER_CONSTANTS,
    subtype: &windows::core::GUID,
    size: Option<(u32, u32)>,
) -> windows::core::Result<IMFSourceReader> {
    let mut attrs = None;
    MFCreateAttributes(&mut attrs, 1)?;
    let attrs = attrs.unwrap();
    // the reader's own conversion from the decoder's YUV to RGB
    attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
    let url = HSTRING::from(path.as_os_str());
    let r = MFCreateSourceReaderFromURL(PCWSTR(url.as_ptr()), &attrs)?;
    r.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
    r.SetStreamSelection(stream.0 as u32, true)?;
    let t = MFCreateMediaType()?;
    let video = stream == MF_SOURCE_READER_FIRST_VIDEO_STREAM;
    t.SetGUID(&MF_MT_MAJOR_TYPE, if video { &MFMediaType_Video } else { &MFMediaType_Audio })?;
    t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
    if !video {
        t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
    }
    if let Some((w, h)) = size {
        t.SetUINT64(&MF_MT_FRAME_SIZE, (w as u64) << 32 | h as u64)?;
    }
    r.SetCurrentMediaType(stream.0 as u32, None, &t)?;
    Ok(r)
}

/// The next sample of the reader's one stream: `None` at the end.
unsafe fn next(
    r: &IMFSourceReader,
    stream: MF_SOURCE_READER_CONSTANTS,
) -> windows::core::Result<Option<(i64, u32, IMFSample)>> {
    loop {
        let (mut flags, mut ts, mut sample) = (0u32, 0i64, None);
        r.ReadSample(stream.0 as u32, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample))?;
        if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
            return Ok(None);
        }
        if let Some(s) = sample {
            return Ok(Some((ts, flags, s)));
        }
    }
}

/// The sound, whole, as 16-bit PCM (silence before its first sample's time).
unsafe fn sound(path: &Path) -> windows::core::Result<Pcm> {
    let stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM;
    let r = reader(path, stream, &MFAudioFormat_PCM, None)?;
    let t = r.GetCurrentMediaType(stream.0 as u32)?;
    let channels = t.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)? as usize;
    let rate = t.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)?;
    let mut samples: Vec<i16> = Vec::new();
    let mut first = true;
    while let Some((ts, _, s)) = next(&r, stream)? {
        if std::mem::take(&mut first) && ts > 0 {
            samples.resize((ts as u128 * rate as u128 / 10_000_000) as usize * channels, 0);
        }
        let buf = s.ConvertToContiguousBuffer()?;
        let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
        buf.Lock(&mut p, None, Some(&mut len))?;
        let bytes = std::slice::from_raw_parts(p, len as usize);
        samples.extend(bytes.as_chunks::<2>().0.iter().map(|&b| i16::from_le_bytes(b)));
        buf.Unlock()?;
    }
    Ok(Pcm { channels, rate, samples })
}

/// The reader's picture size.
unsafe fn picture_size(r: &IMFSourceReader) -> windows::core::Result<(u32, u32)> {
    let t = r.GetCurrentMediaType(MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32)?;
    let s = t.GetUINT64(&MF_MT_FRAME_SIZE)?;
    Ok(((s >> 32) as u32, s as u32))
}

/// A sample's picture (32-bit RGB), rows top down with no padding.
unsafe fn bgra(s: &IMFSample, (w, h): (u32, u32)) -> windows::core::Result<Vec<u8>> {
    let (w, h) = (w as usize, h as usize);
    let buf = s.GetBufferByIndex(0)?;
    let mut out = vec![0u8; w * h * 4];
    let mut copy = |row0: *const u8, pitch: isize| {
        for (y, line) in out.chunks_exact_mut(w * 4).enumerate() {
            line.copy_from_slice(std::slice::from_raw_parts(row0.offset(y as isize * pitch), w * 4));
        }
    };
    if let Ok(b2) = buf.cast::<IMF2DBuffer>() {
        // the pitch is negative for a picture stored bottom up
        let (mut p, mut pitch) = (std::ptr::null_mut(), 0i32);
        b2.Lock2D(&mut p, &mut pitch)?;
        copy(p, pitch as isize);
        b2.Unlock2D()?;
    } else {
        let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
        buf.Lock(&mut p, None, Some(&mut len))?;
        if (len as usize) >= w * h * 4 {
            copy(p, (w * 4) as isize);
        }
        buf.Unlock()?;
    }
    Ok(out)
}

/// The pictures' thread: decodes and sends until the end or until a send fails.
fn pictures(path: PathBuf, size: (u32, u32), tx: SyncSender<Result<Picture, String>>) {
    let run = || unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM;
        let r = reader(&path, stream, &MFVideoFormat_RGB32, Some(size))?;
        let mut size = size;
        while let Some((ts, flags, s)) = next(&r, stream)? {
            if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
                size = picture_size(&r)?;
            }
            let p = Picture { at: ts as f64 / 1e7, bgra: bgra(&s, size)? };
            if tx.send(Ok(p)).is_err() {
                break;
            }
        }
        windows::core::Result::Ok(())
    };
    if let Err(e) = run() {
        tx.send(Err(format!("{}: {e}", path.display()))).ok();
    }
}

/// Open `path`: its sound decoded, its pictures starting on their thread, scaled to fit `within`.
pub fn open(path: &Path, within: (u32, u32)) -> Result<HdMovie, String> {
    startup()?;
    let (size, decoded) = unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok();
        let video = MF_SOURCE_READER_FIRST_VIDEO_STREAM;
        let size = picture_size(&reader(path, video, &MFVideoFormat_RGB32, None).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        // the reader scales when asked; the stored size when it will not
        let decoded = match reader(path, video, &MFVideoFormat_RGB32, Some(fitted(size, within))) {
            Ok(r) => picture_size(&r).unwrap_or(size),
            Err(_) => size,
        };
        (size, decoded)
    };
    let sound = match unsafe { sound(path) } {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("{}: sound: {e}", path.display());
            None
        }
    };
    let owned = path.to_path_buf();
    HdMovie::start(size, decoded, sound, "Media Foundation", move |tx| pictures(owned, decoded, tx))
}
