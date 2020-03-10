extern crate alsa;
extern crate num_traits;
//use std::{iter, error};
//use std::any::{Any, TypeId};
use alsa::pcm::{Access, Format, HwParams, State};
use alsa::{Direction, ValueOr};
use alsa::ctl::{ElemId, ElemIface};
use alsa::ctl::{ElemValue, ElemType};
use std::ffi::CString;
use alsa::hctl::HCtl;
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::SystemTime;
//mod audiodevice;
use audiodevice::*;
// Sample format
use config::SampleFormat;
use conversions::{
    buffer_to_chunk_float, buffer_to_chunk_int, chunk_to_buffer_float, chunk_to_buffer_int,
};

use CommandMessage;
use PrcFmt;
use Res;
use StatusMessage;

#[cfg(target_pointer_width = "64")]
pub type MachInt = i64;
#[cfg(not(target_pointer_width = "64"))]
pub type MachInt = i32;

pub struct AlsaPlaybackDevice {
    pub devname: String,
    pub samplerate: usize,
    pub bufferlength: usize,
    pub channels: usize,
    pub format: SampleFormat,
    pub target_level: usize,
    pub adjust_period: f32,
}

pub struct AlsaCaptureDevice {
    pub devname: String,
    pub samplerate: usize,
    pub bufferlength: usize,
    pub channels: usize,
    pub format: SampleFormat,
    pub silence_threshold: PrcFmt,
    pub silence_timeout: PrcFmt,
}

struct AdjustParams {
    buffersize: usize,
    target_level: usize,
    adjust_period: f32,
}

/// Play a buffer.
fn play_buffer<T: std::marker::Copy>(
    buffer: &[T],
    pcmdevice: &alsa::PCM,
    io: &alsa::pcm::IO<T>,
) -> Res<()> {
    let playback_state = pcmdevice.state();
    //eprintln!("playback state {:?}", playback_state);
    if playback_state == State::XRun {
        eprintln!("Prepare playback");
        pcmdevice.prepare()?;
    }
    let _frames = match io.writei(&buffer[..]) {
        Ok(frames) => frames,
        Err(_err) => {
            eprintln!("retrying playback");
            pcmdevice.prepare()?;
            io.writei(&buffer[..])?
        }
    };
    Ok(())
}

/// Play a buffer.
fn capture_buffer<T: std::marker::Copy>(
    buffer: &mut [T],
    pcmdevice: &alsa::PCM,
    io: &alsa::pcm::IO<T>,
) -> Res<()> {
    let capture_state = pcmdevice.state();
    if capture_state == State::XRun {
        eprintln!("prepare capture");
        pcmdevice.prepare()?;
    }
    let _frames = match io.readi(buffer) {
        Ok(frames) => frames,
        Err(_err) => {
            eprintln!("retrying capture");
            pcmdevice.prepare()?;
            io.readi(buffer)?
        }
    };
    Ok(())
}

/// Open an Alsa PCM device
fn open_pcm(
    devname: String,
    samplerate: u32,
    bufsize: MachInt,
    channels: u32,
    format: &SampleFormat,
    capture: bool,
) -> Res<alsa::PCM> {
    // Open the device
    let pcmdev;
    if capture {
        pcmdev = alsa::PCM::new(&devname, Direction::Capture, false)?;
    } else {
        pcmdev = alsa::PCM::new(&devname, Direction::Playback, false)?;
    }
    // Set hardware parameters
    {
        let hwp = HwParams::any(&pcmdev)?;
        hwp.set_channels(channels)?;
        hwp.set_rate(samplerate, ValueOr::Nearest)?;
        match format {
            SampleFormat::S16LE => hwp.set_format(Format::s16())?,
            SampleFormat::S24LE => hwp.set_format(Format::s24())?,
            SampleFormat::S32LE => hwp.set_format(Format::s32())?,
            SampleFormat::FLOAT32LE => hwp.set_format(Format::float())?,
            SampleFormat::FLOAT64LE => hwp.set_format(Format::float64())?,
        }

        hwp.set_access(Access::RWInterleaved)?;
        hwp.set_buffer_size(2*bufsize)?;
        hwp.set_period_size(bufsize / 8, alsa::ValueOr::Nearest)?;
        pcmdev.hw_params(&hwp)?;
    }

    // Set software parameters
    let (_rate, _act_bufsize) = {
        let hwp = pcmdev.hw_params_current()?;
        let swp = pcmdev.sw_params_current()?;
        let (act_bufsize, act_periodsize) = (hwp.get_buffer_size()?, hwp.get_period_size()?);
        swp.set_start_threshold(act_bufsize/2 - act_periodsize)?;
        //swp.set_avail_min(periodsize)?;
        pcmdev.sw_params(&swp)?;
        //eprintln!("Opened audio output {:?} with parameters: {:?}, {:?}", devname, hwp, swp);
        (hwp.get_rate()?, act_bufsize)
    };
    Ok(pcmdev)
}

fn playback_loop_int<T: num_traits::NumCast + std::marker::Copy>(
    channel: mpsc::Receiver<AudioMessage>,
    status_channel: mpsc::Sender<StatusMessage>,
    mut buffer: Vec<T>,
    pcmdevice: &alsa::PCM,
    io: alsa::pcm::IO<T>,
    scalefactor: PrcFmt,
    chunksize: usize,
    target_fill: usize,
    adjust_period: f32,
) {
    let srate = pcmdevice.hw_params_current().unwrap().get_rate().unwrap();
    let mut start = SystemTime::now();
    let mut now;
    let mut delay = 0;
    let mut ndelays = 0;
    let mut speed;
    let mut diff: isize;
    loop {
        match channel.recv() {
            Ok(AudioMessage::Audio(chunk)) => {
                chunk_to_buffer_int(chunk, &mut buffer, scalefactor);
                now = SystemTime::now();
                delay += pcmdevice.status().unwrap().get_delay() as isize;
                ndelays += 1;
                if now.duration_since(start).unwrap().as_millis() > 1000 {
                    let av_delay = delay/ndelays;
                    diff = av_delay - target_fill as isize;
                    let rel_diff = (diff as f32) / (srate as f32);
                    speed = 1.0 + rel_diff; 
                    eprintln!("set speed to {}", speed);
                    eprintln!("rate: {}, bufsize: {}, current delay {}", srate, chunksize, av_delay);
                    start = now;
                    delay=0;
                    ndelays=0;
                    status_channel.send(StatusMessage::SetSpeed { speed }).unwrap();
                }
                
                let playback_res = play_buffer(&buffer, pcmdevice, &io);
                match playback_res {
                    Ok(_) => {}
                    Err(msg) => {
                        status_channel
                            .send(StatusMessage::PlaybackError {
                                message: format!("{}", msg),
                            })
                            .unwrap();
                    }
                };
            }
            Ok(AudioMessage::EndOfStream) => {
                status_channel.send(StatusMessage::PlaybackDone).unwrap();
                break;
            }
            _ => {}
        }
    }
}

fn playback_loop_float<T: num_traits::NumCast + std::marker::Copy>(
    channel: mpsc::Receiver<AudioMessage>,
    status_channel: mpsc::Sender<StatusMessage>,
    mut buffer: Vec<T>,
    pcmdevice: &alsa::PCM,
    io: alsa::pcm::IO<T>,
    chunksize: usize,
) {
    loop {
        match channel.recv() {
            Ok(AudioMessage::Audio(chunk)) => {
                chunk_to_buffer_float(chunk, &mut buffer);
                let delay = pcmdevice.status().unwrap().get_delay();
                eprintln!("current delay {}", delay);
                let playback_res = play_buffer(&buffer, &pcmdevice, &io);
                match playback_res {
                    Ok(_) => {}
                    Err(msg) => {
                        status_channel
                            .send(StatusMessage::PlaybackError {
                                message: format!("{}", msg),
                            })
                            .unwrap();
                    }
                };
            }
            Ok(AudioMessage::EndOfStream) => {
                status_channel.send(StatusMessage::PlaybackDone).unwrap();
                break;
            }
            _ => {}
        }
    }
}

fn capture_loop_int<
    T: num_traits::NumCast + std::marker::Copy + num_traits::AsPrimitive<PrcFmt>,
>(
    msg_channels: (
        mpsc::SyncSender<AudioMessage>,
        mpsc::Sender<StatusMessage>,
        mpsc::Receiver<CommandMessage>,
    ),
    mut buffer: Vec<T>,
    pcmdevice: &alsa::PCM,
    io: alsa::pcm::IO<T>,
    capt_params: (usize, PrcFmt, usize, PrcFmt),
) {
    let mut silent_nbr: usize = 0;
    let (channel, status_channel, command_channel) = msg_channels;
    let (channels, scalefactor, silent_limit, silence) = capt_params;
    let pcminfo = pcmdevice.info().unwrap();
    let card = pcminfo.get_card();
    let device = pcminfo.get_device();
    let subdevice = pcminfo.get_subdevice();
    let mut elid = ElemId::new(ElemIface::PCM);
    elid.set_device(device);
    elid.set_subdevice(subdevice);
    elid.set_name(&CString::new("PCM Rate Shift 100000").unwrap());
    let h = HCtl::new(&format!("hw:{}", card), false).unwrap();
    h.load().unwrap();
    let element = h.find_elem(&elid);
    let mut elval = ElemValue::new(ElemType::Integer).unwrap();

    loop {
        match command_channel.try_recv() {
            Ok(CommandMessage::Exit) => {
                let msg = AudioMessage::EndOfStream;
                channel.send(msg).unwrap();
                status_channel.send(StatusMessage::CaptureDone).unwrap();
                break;
            },
            Ok(CommandMessage::SetSpeed{ speed }) => {
                eprintln!("cap setting speed to {}", speed);
                if let Some(elem) = &element {
                    elval.set_integer(0, (100000.0*speed) as i32).unwrap();
                    elem.write(&elval).unwrap();
                }
            },
            Err(_) => {}
        };
        let capture_res = capture_buffer(&mut buffer, pcmdevice, &io);
        match capture_res {
            Ok(_) => {}
            Err(msg) => {
                status_channel
                    .send(StatusMessage::CaptureError {
                        message: format!("{}", msg),
                    })
                    .unwrap();
            }
        };
        let chunk = buffer_to_chunk_int(&buffer, channels, scalefactor);
        if (chunk.maxval - chunk.minval) > silence {
            if silent_nbr > silent_limit {
                eprintln!("Resuming processing");
            }
            silent_nbr = 0;
        } else if silent_limit > 0 {
            if silent_nbr == silent_limit {
                eprintln!("Pausing processing");
            }
            silent_nbr += 1;
        }
        if silent_nbr <= silent_limit {
            let msg = AudioMessage::Audio(chunk);
            channel.send(msg).unwrap();
        }
    }
}

fn capture_loop_float<
    T: num_traits::NumCast + std::marker::Copy + num_traits::AsPrimitive<PrcFmt>,
>(
    msg_channels: (
        mpsc::SyncSender<AudioMessage>,
        mpsc::Sender<StatusMessage>,
        mpsc::Receiver<CommandMessage>,
    ),
    mut buffer: Vec<T>,
    pcmdevice: &alsa::PCM,
    io: alsa::pcm::IO<T>,
    capt_params: (usize, usize, PrcFmt),
) {
    let mut silent_nbr: usize = 0;
    let (channel, status_channel, command_channel) = msg_channels;
    let (channels, silent_limit, silence) = capt_params;
    loop {
        if let Ok(CommandMessage::Exit) = command_channel.try_recv() {
            let msg = AudioMessage::EndOfStream;
            channel.send(msg).unwrap();
            status_channel.send(StatusMessage::CaptureDone).unwrap();
            break;
        }
        let capture_res = capture_buffer(&mut buffer, pcmdevice, &io);
        match capture_res {
            Ok(_) => {}
            Err(msg) => {
                status_channel
                    .send(StatusMessage::CaptureError {
                        message: format!("{}", msg),
                    })
                    .unwrap();
            }
        };
        let chunk = buffer_to_chunk_float(&buffer, channels);
        if (chunk.maxval - chunk.minval) > silence {
            if silent_nbr > silent_limit {
                eprintln!("Resuming processing");
            }
            silent_nbr = 0;
        } else if silent_limit > 0 {
            if silent_nbr == silent_limit {
                eprintln!("Pausing processing");
            }
            silent_nbr += 1;
        }
        if silent_nbr <= silent_limit {
            let msg = AudioMessage::Audio(chunk);
            channel.send(msg).unwrap();
        }
    }
}

/// Start a playback thread listening for AudioMessages via a channel.
impl PlaybackDevice for AlsaPlaybackDevice {
    fn start(
        &mut self,
        channel: mpsc::Receiver<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: mpsc::Sender<StatusMessage>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let devname = self.devname.clone();
        let target_level = self.target_level;
        let adjust_period = self.adjust_period;
        let samplerate = self.samplerate;
        let bufferlength = self.bufferlength;
        let channels = self.channels;
        let bits = match self.format {
            SampleFormat::S16LE => 16,
            SampleFormat::S24LE => 24,
            SampleFormat::S32LE => 32,
            SampleFormat::FLOAT32LE => 32,
            SampleFormat::FLOAT64LE => 64,
        };
        let format = self.format.clone();
        let handle = thread::spawn(move || {
            //let delay = time::Duration::from_millis((4*1000*bufferlength/samplerate) as u64);
            match open_pcm(
                devname,
                samplerate as u32,
                bufferlength as MachInt,
                channels as u32,
                &format,
                false,
            ) {
                Ok(pcmdevice) => {
                    match status_channel.send(StatusMessage::PlaybackReady) {
                        Ok(()) => {}
                        Err(_err) => {}
                    }
                    //let scalefactor = (1<<bits-1) as PrcFmt;
                    let scalefactor = (2.0 as PrcFmt).powf((bits - 1) as PrcFmt);

                    barrier.wait();
                    //thread::sleep(delay);
                    eprintln!("starting playback loop");
                    match format {
                        SampleFormat::S16LE => {
                            let io = pcmdevice.io_i16().unwrap();
                            let buffer = vec![0i16; bufferlength * channels];
                            playback_loop_int(
                                channel,
                                status_channel,
                                buffer,
                                &pcmdevice,
                                io,
                                scalefactor,
                                bufferlength,
                                target_level,
                                adjust_period,
                            );
                        }
                        SampleFormat::S24LE | SampleFormat::S32LE => {
                            let io = pcmdevice.io_i32().unwrap();
                            let buffer = vec![0i32; bufferlength * channels];
                            playback_loop_int(
                                channel,
                                status_channel,
                                buffer,
                                &pcmdevice,
                                io,
                                scalefactor,
                                bufferlength,
                                target_level,
                                adjust_period,
                            );
                        }
                        SampleFormat::FLOAT32LE => {
                            let io = pcmdevice.io_f32().unwrap();
                            let buffer = vec![0f32; bufferlength * channels];
                            playback_loop_float(channel, status_channel, buffer, &pcmdevice, io, bufferlength);
                        }
                        SampleFormat::FLOAT64LE => {
                            let io = pcmdevice.io_f64().unwrap();
                            let buffer = vec![0f64; bufferlength * channels];
                            playback_loop_float(channel, status_channel, buffer, &pcmdevice, io, bufferlength);
                        }
                    };
                }
                Err(err) => {
                    status_channel
                        .send(StatusMessage::PlaybackError {
                            message: format!("{}", err),
                        })
                        .unwrap();
                }
            }
        });
        Ok(Box::new(handle))
    }
}

/// Start a capture thread providing AudioMessages via a channel
impl CaptureDevice for AlsaCaptureDevice {
    fn start(
        &mut self,
        channel: mpsc::SyncSender<AudioMessage>,
        barrier: Arc<Barrier>,
        status_channel: mpsc::Sender<StatusMessage>,
        command_channel: mpsc::Receiver<CommandMessage>,
    ) -> Res<Box<thread::JoinHandle<()>>> {
        let devname = self.devname.clone();
        let samplerate = self.samplerate;
        let bufferlength = self.bufferlength;
        let channels = self.channels;
        let bits = match self.format {
            SampleFormat::S16LE => 16,
            SampleFormat::S24LE => 24,
            SampleFormat::S32LE => 32,
            SampleFormat::FLOAT32LE => 32,
            SampleFormat::FLOAT64LE => 64,
        };
        let mut silence: PrcFmt = 10.0;
        silence = silence.powf(self.silence_threshold / 20.0);
        let silent_limit =
            (self.silence_timeout * ((samplerate / bufferlength) as PrcFmt)) as usize;
        let format = self.format.clone();
        let handle = thread::spawn(move || {
            match open_pcm(
                devname,
                samplerate as u32,
                bufferlength as MachInt,
                channels as u32,
                &format,
                true,
            ) {
                Ok(pcmdevice) => {
                    match status_channel.send(StatusMessage::CaptureReady) {
                        Ok(()) => {}
                        Err(_err) => {}
                    }
                    let scalefactor = (2.0 as PrcFmt).powf((bits - 1) as PrcFmt);
                    barrier.wait();
                    eprintln!("starting captureloop");
                    match format {
                        SampleFormat::S16LE => {
                            let msg_channels = (channel, status_channel, command_channel);
                            let capt_params = (channels, scalefactor, silent_limit, silence);
                            let io = pcmdevice.io_i16().unwrap();
                            let buffer = vec![0i16; channels * bufferlength];
                            capture_loop_int(msg_channels, buffer, &pcmdevice, io, capt_params);
                        }
                        SampleFormat::S24LE | SampleFormat::S32LE => {
                            let msg_channels = (channel, status_channel, command_channel);
                            let capt_params = (channels, scalefactor, silent_limit, silence);
                            let io = pcmdevice.io_i32().unwrap();
                            let buffer = vec![0i32; channels * bufferlength];
                            capture_loop_int(msg_channels, buffer, &pcmdevice, io, capt_params);
                        }
                        SampleFormat::FLOAT32LE => {
                            let msg_channels = (channel, status_channel, command_channel);
                            let capt_params = (channels, silent_limit, silence);
                            let io = pcmdevice.io_f32().unwrap();
                            let buffer = vec![0f32; channels * bufferlength];
                            capture_loop_float(msg_channels, buffer, &pcmdevice, io, capt_params);
                        }
                        SampleFormat::FLOAT64LE => {
                            let msg_channels = (channel, status_channel, command_channel);
                            let capt_params = (channels, silent_limit, silence);
                            let io = pcmdevice.io_f64().unwrap();
                            let buffer = vec![0f64; channels * bufferlength];
                            capture_loop_float(msg_channels, buffer, &pcmdevice, io, capt_params);
                        }
                    };
                }
                Err(err) => {
                    status_channel
                        .send(StatusMessage::CaptureError {
                            message: format!("{}", err),
                        })
                        .unwrap();
                }
            }
        });
        Ok(Box::new(handle))
    }
}
