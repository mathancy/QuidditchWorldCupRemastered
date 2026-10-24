//! Playback through the system's default output device (`cpal`), with the mixer rendering in the device callback.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};

use crate::mixer::Mixer;
use crate::{Error, Result};

/// An open output stream (or none: [`Output::offline`]). Dropping it stops playback.
pub struct Output {
    _stream: Option<cpal::Stream>,
    mixer: Arc<Mutex<Mixer>>,
    pub device: String,
    pub channels: u16,
}

fn playback(e: impl std::fmt::Display) -> Error {
    Error::Playback(e.to_string())
}

impl Output {
    /// Open the default output device at its default rate and start the stream.
    pub fn open() -> Result<Output> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| playback("no audio output device"))?;
        let name = device.id().map(|id| id.to_string()).unwrap_or_else(|_| "default".into());
        let supported = device.default_output_config().map_err(playback)?;
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let mixer = Arc::new(Mutex::new(Mixer::new(config.sample_rate)));
        let channels = config.channels;
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, config, &mixer),
            SampleFormat::I16 => build::<i16>(&device, config, &mixer),
            SampleFormat::U16 => build::<u16>(&device, config, &mixer),
            SampleFormat::I32 => build::<i32>(&device, config, &mixer),
            SampleFormat::F64 => build::<f64>(&device, config, &mixer),
            f => return Err(playback(format!("unsupported sample format {f}"))),
        }?;
        stream.play().map_err(playback)?;
        Ok(Output { _stream: Some(stream), mixer, device: name, channels })
    }

    /// A mixer at `rate` that no device plays: the caller renders it (`Mixer::render`), for checks and files.
    pub fn offline(rate: u32) -> Output {
        Output { _stream: None, mixer: Arc::new(Mutex::new(Mixer::new(rate))), device: "offline".into(), channels: 2 }
    }

    /// The mixer the stream renders. Hold its lock only briefly: the device callback waits on it.
    pub fn mixer(&self) -> &Arc<Mutex<Mixer>> {
        &self.mixer
    }
}

fn build<T>(device: &cpal::Device, config: StreamConfig, mixer: &Arc<Mutex<Mixer>>) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mixer = mixer.clone();
    let mut stereo: Vec<f32> = Vec::new();
    let data = move |out: &mut [T], _: &cpal::OutputCallbackInfo| {
        let frames = out.len() / channels.max(1);
        stereo.resize(frames * 2, 0.0);
        mixer.lock().unwrap().render(&mut stereo);
        for (o, s) in out.chunks_mut(channels.max(1)).zip(stereo.as_chunks::<2>().0) {
            match o.len() {
                1 => o[0] = T::from_sample(0.5 * (s[0] + s[1])),
                _ => {
                    o[0] = T::from_sample(s[0]);
                    o[1] = T::from_sample(s[1]);
                    o[2..].fill(T::from_sample(0.0));
                }
            }
        }
    };
    let error = |e: cpal::Error| eprintln!("audio output: {e}");
    device.build_output_stream(config, data, error, None).map_err(playback)
}
