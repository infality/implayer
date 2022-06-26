use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{
        mpsc::{Receiver, Sender},
        Arc, Mutex,
    },
};

use symphonia::core::{
    codecs::Decoder,
    formats::{FormatReader, SeekMode, SeekTo},
    units::{Time, TimeBase},
};

use crate::output;

fn time_to_ms(time: Time) -> u64 {
    time.seconds * 1000 + (time.frac * 1000.0) as u64
}

fn ms_to_time(ms: u64) -> Time {
    Time {
        seconds: ms / 1000,
        frac: (ms % 1000) as f64 / 1000.0,
    }
}

pub fn get_duration(path: &Path) -> u64 {
    let mss = symphonia::core::io::MediaSourceStream::new(
        Box::new(File::open(path).unwrap()),
        Default::default(),
    );
    let reader = symphonia::default::get_probe()
        .format(
            &Default::default(),
            mss,
            &Default::default(),
            &Default::default(),
        )
        .unwrap()
        .format;

    let track = reader.tracks().first().unwrap();

    let tb = track.codec_params.time_base.unwrap();
    time_to_ms(
        tb.calc_time(
            track
                .codec_params
                .n_frames
                .map(|frames| track.codec_params.start_ts + frames)
                .unwrap(),
        ),
    )
}

pub enum PlayerAction {
    Play(PathBuf),
    Pause,
    Resume,
    Stop,
    Seek(u64),
    SetVolume(f32),
}

pub fn run(
    action_rx: Receiver<PlayerAction>,
    song_ended_tx: Sender<()>,
    position: Arc<Mutex<u64>>,
) {
    struct PlayerState {
        reader: Box<dyn FormatReader>,
        audio_output: Option<Box<dyn output::AudioOutput>>,
        decoder: Box<dyn Decoder>,
        time_base: TimeBase,
    }

    let mut state = None;
    let mut is_playing = false;
    let mut volume = 0.93_f32.powi(4);

    loop {
        let result = if is_playing {
            match action_rx.try_recv() {
                Ok(action) => Some(action),
                Err(_) => None,
            }
        } else {
            match action_rx.recv() {
                Ok(action) => Some(action),
                Err(_) => return,
            }
        };

        match result {
            Some(PlayerAction::Play(path)) => {
                let mss = symphonia::core::io::MediaSourceStream::new(
                    Box::new(File::open(path).unwrap()),
                    Default::default(),
                );
                let reader = symphonia::default::get_probe()
                    .format(
                        &Default::default(),
                        mss,
                        &symphonia::core::formats::FormatOptions {
                            enable_gapless: true,
                            ..Default::default()
                        },
                        &Default::default(),
                    )
                    .unwrap()
                    .format;

                let track = reader.tracks().first().unwrap();
                let decoder = symphonia::default::get_codecs()
                    .make(
                        &track.codec_params,
                        &symphonia::core::codecs::DecoderOptions { verify: false },
                    )
                    .unwrap();
                let time_base = track.codec_params.time_base.unwrap();

                state = Some(PlayerState {
                    reader,
                    audio_output: None,
                    decoder,
                    time_base,
                });
                is_playing = true;
            }
            Some(PlayerAction::Pause) => {
                if state.is_some() {
                    is_playing = false;
                }
            }
            Some(PlayerAction::Resume) => {
                if state.is_some() {
                    is_playing = true;
                }
            }
            Some(PlayerAction::Stop) => {
                state = None;
                is_playing = false;
            }
            Some(PlayerAction::Seek(ms)) => {
                if state.is_some() {
                    state
                        .as_mut()
                        .unwrap()
                        .reader
                        .seek(
                            SeekMode::Accurate,
                            SeekTo::Time {
                                time: ms_to_time(ms),
                                track_id: None,
                            },
                        )
                        .unwrap();
                }
            }
            Some(PlayerAction::SetVolume(v)) => {
                volume = v;
            }
            None => (),
        }

        if state.is_none() || !is_playing {
            continue;
        }

        let s = state.as_mut().unwrap();

        let packet = match s.reader.next_packet() {
            Ok(packet) => packet,
            Err(_) => {
                state = None;
                is_playing = false;
                song_ended_tx.send(()).unwrap();
                continue;
            }
        };

        match s.decoder.decode(&packet) {
            Ok(decoded) => {
                if s.audio_output.is_none() {
                    let spec = *decoded.spec();
                    let duration = decoded.capacity() as u64;
                    s.audio_output
                        .replace(output::try_open(spec, duration).unwrap());
                }

                *position.lock().unwrap() = time_to_ms(s.time_base.calc_time(packet.ts()));

                if let Some(ref mut audio_output) = s.audio_output {
                    audio_output.write(decoded, volume).unwrap()
                }
            }
            Err(symphonia::core::errors::Error::DecodeError(err)) => {
                println!("decode error: {}", err);
            }
            Err(_) => {
                state = None;
                is_playing = false;
                song_ended_tx.send(()).unwrap();
            }
        }
    }
}
