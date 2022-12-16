use clap::crate_version;
#[cfg(feature = "secure-websocket")]
use native_tls::{Identity, TlsAcceptor, TlsStream};
use serde::{Deserialize, Serialize};
#[cfg(feature = "secure-websocket")]
use std::fs::File;
#[cfg(feature = "secure-websocket")]
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::accept;
use tungstenite::Message;
use tungstenite::WebSocket;

use crate::config;
use crate::helpers::linear_to_db;
use crate::ExitRequest;
use crate::ProcessingState;
use crate::Res;
use crate::{
    list_supported_devices, CaptureStatus, PlaybackStatus, ProcessingParameters, ProcessingStatus,
    StopReason,
};

#[derive(Debug, Clone)]
pub struct SharedData {
    pub signal_reload: Arc<AtomicBool>,
    pub signal_exit: Arc<AtomicUsize>,
    pub active_config: Arc<Mutex<Option<config::Configuration>>>,
    pub active_config_path: Arc<Mutex<Option<String>>>,
    pub new_config: Arc<Mutex<Option<config::Configuration>>>,
    pub previous_config: Arc<Mutex<Option<config::Configuration>>>,
    pub capture_status: Arc<Mutex<CaptureStatus>>,
    pub playback_status: Arc<Mutex<PlaybackStatus>>,
    pub processing_status: Arc<Mutex<ProcessingParameters>>,
    pub status: Arc<Mutex<ProcessingStatus>>,
}

#[derive(Debug, Clone)]
pub struct LocalData {
    pub last_cap_rms_time: Instant,
    pub last_cap_peak_time: Instant,
    pub last_pb_rms_time: Instant,
    pub last_pb_peak_time: Instant,
}

#[derive(Debug, Clone)]
pub struct ServerParameters<'a> {
    pub address: &'a str,
    pub port: usize,
    #[cfg(feature = "secure-websocket")]
    pub cert_file: Option<&'a str>,
    #[cfg(feature = "secure-websocket")]
    pub cert_pass: Option<&'a str>,
}

#[derive(Debug, PartialEq, Deserialize)]
enum WsCommand {
    SetConfigName(String),
    SetConfig(String),
    SetConfigJson(String),
    Reload,
    GetConfig,
    GetConfigTitle,
    GetConfigDescription,
    GetPreviousConfig,
    ReadConfig(String),
    ReadConfigFile(String),
    ValidateConfig(String),
    GetConfigJson,
    GetConfigName,
    GetSignalRange,
    GetCaptureSignalRms,
    GetCaptureSignalRmsSince(f32),
    GetCaptureSignalRmsSinceLast,
    GetCaptureSignalPeak,
    GetCaptureSignalPeakSince(f32),
    GetCaptureSignalPeakSinceLast,
    GetPlaybackSignalRms,
    GetPlaybackSignalRmsSince(f32),
    GetPlaybackSignalRmsSinceLast,
    GetPlaybackSignalPeak,
    GetPlaybackSignalPeakSince(f32),
    GetPlaybackSignalPeakSinceLast,
    GetSignalLevels,
    GetSignalLevelsSince(f32),
    GetSignalLevelsSinceLast,
    GetSignalPeaksSinceStart,
    ResetSignalPeaksSinceStart,
    GetCaptureRate,
    GetUpdateInterval,
    SetUpdateInterval(usize),
    GetVolume,
    SetVolume(f32),
    AdjustVolume(f32),
    GetMute,
    SetMute(bool),
    ToggleMute,
    GetFaderVolume(usize),
    SetFaderVolume(usize, f32),
    SetFaderExternalVolume(usize, f32),
    AdjustFaderVolume(usize, f32),
    GetFaderMute(usize),
    SetFaderMute(usize, bool),
    ToggleFaderMute(usize),
    GetVersion,
    GetState,
    GetStopReason,
    GetRateAdjust,
    GetClippedSamples,
    ResetClippedSamples,
    GetBufferLevel,
    GetSupportedDeviceTypes,
    Exit,
    Stop,
    None,
}

#[derive(Debug, Eq, PartialEq, Serialize)]
enum WsResult {
    Ok,
    Error,
}

#[derive(Debug, PartialEq, Serialize)]
struct AllLevels {
    playback_rms: Vec<f32>,
    playback_peak: Vec<f32>,
    capture_rms: Vec<f32>,
    capture_peak: Vec<f32>,
}

#[derive(Debug, PartialEq, Serialize)]
struct PbCapLevels {
    playback: Vec<f32>,
    capture: Vec<f32>,
}

#[derive(Debug, PartialEq, Serialize)]
enum WsReply {
    SetConfigName {
        result: WsResult,
    },
    SetConfig {
        result: WsResult,
    },
    SetConfigJson {
        result: WsResult,
    },
    Reload {
        result: WsResult,
    },
    GetConfig {
        result: WsResult,
        value: String,
    },
    GetConfigTitle {
        result: WsResult,
        value: String,
    },
    GetConfigDescription {
        result: WsResult,
        value: String,
    },
    GetPreviousConfig {
        result: WsResult,
        value: String,
    },
    ReadConfig {
        result: WsResult,
        value: String,
    },
    ReadConfigFile {
        result: WsResult,
        value: String,
    },
    ValidateConfig {
        result: WsResult,
        value: String,
    },
    GetConfigJson {
        result: WsResult,
        value: String,
    },
    GetConfigName {
        result: WsResult,
        value: String,
    },
    GetSignalRange {
        result: WsResult,
        value: f32,
    },
    GetPlaybackSignalRms {
        result: WsResult,
        value: Vec<f32>,
    },
    GetPlaybackSignalRmsSince {
        result: WsResult,
        value: Vec<f32>,
    },
    GetPlaybackSignalRmsSinceLast {
        result: WsResult,
        value: Vec<f32>,
    },
    GetPlaybackSignalPeak {
        result: WsResult,
        value: Vec<f32>,
    },
    GetPlaybackSignalPeakSince {
        result: WsResult,
        value: Vec<f32>,
    },
    GetPlaybackSignalPeakSinceLast {
        result: WsResult,
        value: Vec<f32>,
    },
    GetCaptureSignalRms {
        result: WsResult,
        value: Vec<f32>,
    },
    GetCaptureSignalRmsSince {
        result: WsResult,
        value: Vec<f32>,
    },
    GetCaptureSignalRmsSinceLast {
        result: WsResult,
        value: Vec<f32>,
    },
    GetCaptureSignalPeak {
        result: WsResult,
        value: Vec<f32>,
    },
    GetCaptureSignalPeakSince {
        result: WsResult,
        value: Vec<f32>,
    },
    GetCaptureSignalPeakSinceLast {
        result: WsResult,
        value: Vec<f32>,
    },
    GetSignalLevels {
        result: WsResult,
        value: AllLevels,
    },
    GetSignalLevelsSince {
        result: WsResult,
        value: AllLevels,
    },
    GetSignalLevelsSinceLast {
        result: WsResult,
        value: AllLevels,
    },
    GetSignalPeaksSinceStart {
        result: WsResult,
        value: PbCapLevels,
    },
    ResetSignalPeaksSinceStart {
        result: WsResult,
    },
    GetCaptureRate {
        result: WsResult,
        value: usize,
    },
    GetUpdateInterval {
        result: WsResult,
        value: usize,
    },
    SetUpdateInterval {
        result: WsResult,
    },
    SetVolume {
        result: WsResult,
    },
    GetVolume {
        result: WsResult,
        value: f32,
    },
    AdjustVolume {
        result: WsResult,
        value: f32,
    },
    SetMute {
        result: WsResult,
    },
    GetMute {
        result: WsResult,
        value: bool,
    },
    ToggleMute {
        result: WsResult,
        value: bool,
    },
    SetFaderVolume {
        result: WsResult,
        control: usize,
    },
    SetFaderExternalVolume {
        result: WsResult,
        control: usize,
    },
    GetFaderVolume {
        result: WsResult,
        value: f32,
        control: usize,
    },
    AdjustFaderVolume {
        result: WsResult,
        value: f32,
        control: usize,
    },
    SetFaderMute {
        result: WsResult,
        control: usize,
    },
    GetFaderMute {
        result: WsResult,
        value: bool,
        control: usize,
    },
    ToggleFaderMute {
        result: WsResult,
        value: bool,
        control: usize,
    },
    GetVersion {
        result: WsResult,
        value: String,
    },
    GetState {
        result: WsResult,
        value: ProcessingState,
    },
    GetStopReason {
        result: WsResult,
        value: StopReason,
    },
    GetRateAdjust {
        result: WsResult,
        value: f32,
    },
    GetBufferLevel {
        result: WsResult,
        value: usize,
    },
    GetClippedSamples {
        result: WsResult,
        value: usize,
    },
    ResetClippedSamples {
        result: WsResult,
    },
    GetSupportedDeviceTypes {
        result: WsResult,
        value: (Vec<String>, Vec<String>),
    },
    Exit {
        result: WsResult,
    },
    Stop {
        result: WsResult,
    },
    Invalid {
        error: String,
    },
}

fn parse_command(cmd: Message) -> Res<WsCommand> {
    match cmd {
        Message::Text(command_str) => {
            let command = serde_json::from_str::<WsCommand>(&command_str)?;
            Ok(command)
        }
        _ => Ok(WsCommand::None),
    }
}

#[cfg(feature = "secure-websocket")]
fn make_acceptor_with_cert(cert: &str, key: &str) -> Res<Arc<TlsAcceptor>> {
    let mut file = File::open(cert)?;
    let mut identity = vec![];
    file.read_to_end(&mut identity)?;
    let identity = Identity::from_pkcs12(&identity, key)?;
    let acceptor = TlsAcceptor::new(identity)?;
    Ok(Arc::new(acceptor))
}

#[cfg(feature = "secure-websocket")]
fn make_acceptor(cert_file: &Option<&str>, cert_key: &Option<&str>) -> Option<Arc<TlsAcceptor>> {
    if let (Some(cert), Some(key)) = (cert_file, cert_key) {
        let acceptor = make_acceptor_with_cert(cert, key);
        match acceptor {
            Ok(acc) => {
                debug!("Created TLS acceptor");
                return Some(acc);
            }
            Err(err) => {
                error!("Could not create TLS acceptor: {}", err);
            }
        }
    }
    debug!("Running websocket server without TLS");
    None
}

pub fn start_server(parameters: ServerParameters, shared_data: SharedData) {
    let address = parameters.address.to_string();
    let port = parameters.port;
    debug!("Start websocket server on {}:{}", address, parameters.port);
    #[cfg(feature = "secure-websocket")]
    let acceptor = make_acceptor(&parameters.cert_file, &parameters.cert_pass);

    thread::spawn(move || {
        let ws_result = TcpListener::bind(format!("{}:{}", address, port));
        if let Ok(server) = ws_result {
            for stream in server.incoming() {
                let shared_data_inst = shared_data.clone();
                let now = Instant::now();
                let local_data = LocalData {
                    last_cap_peak_time: now,
                    last_cap_rms_time: now,
                    last_pb_peak_time: now,
                    last_pb_rms_time: now,
                };
                #[cfg(feature = "secure-websocket")]
                let acceptor_inst = acceptor.clone();

                #[cfg(feature = "secure-websocket")]
                thread::spawn(move || match acceptor_inst {
                    None => {
                        let websocket_res = accept_plain_stream(stream);
                        handle_tcp(websocket_res, &shared_data_inst, local_data);
                    }
                    Some(acc) => {
                        let websocket_res = accept_secure_stream(acc, stream);
                        handle_tls(websocket_res, &shared_data_inst, local_data);
                    }
                });
                #[cfg(not(feature = "secure-websocket"))]
                thread::spawn(move || {
                    let websocket_res = accept_plain_stream(stream);
                    handle_tcp(websocket_res, &shared_data_inst, local_data);
                });
            }
        } else if let Err(err) = ws_result {
            error!("Failed to start websocket server: {}", err);
        }
    });
}

macro_rules! make_handler {
    ($t:ty, $n:ident) => {
        fn $n(
            websocket_res: Res<WebSocket<$t>>,
            shared_data_inst: &SharedData,
            mut local_data: LocalData,
        ) {
            match websocket_res {
                Ok(mut websocket) => loop {
                    let msg_res = websocket.read_message();
                    match msg_res {
                        Ok(msg) => {
                            trace!("received: {:?}", msg);
                            let command = parse_command(msg);
                            debug!("parsed command: {:?}", command);
                            let reply = match command {
                                Ok(cmd) => handle_command(cmd, &shared_data_inst, &mut local_data),
                                Err(err) => Some(WsReply::Invalid {
                                    error: err.to_string(),
                                }),
                            };
                            if let Some(rep) = reply {
                                let write_result = websocket.write_message(Message::text(
                                    serde_json::to_string(&rep).unwrap(),
                                ));
                                if let Err(err) = write_result {
                                    warn!("Failed to write: {}", err);
                                    break;
                                }
                            } else {
                                debug!("Sending no reply");
                            }
                        }
                        Err(tungstenite::error::Error::ConnectionClosed) => {
                            debug!("Connection was closed");
                            break;
                        }
                        Err(err) => {
                            warn!("Lost connection: {}", err);
                            break;
                        }
                    }
                },
                Err(err) => warn!("Connection failed: {}", err),
            };
        }
    };
}

make_handler!(TcpStream, handle_tcp);
#[cfg(feature = "secure-websocket")]
make_handler!(TlsStream<TcpStream>, handle_tls);

#[cfg(feature = "secure-websocket")]
fn accept_secure_stream(
    acceptor: Arc<TlsAcceptor>,
    stream: Result<TcpStream, std::io::Error>,
) -> Res<tungstenite::WebSocket<TlsStream<TcpStream>>> {
    let ws = accept(acceptor.accept(stream?)?)?;
    Ok(ws)
}

fn accept_plain_stream(
    stream: Result<TcpStream, std::io::Error>,
) -> Res<tungstenite::WebSocket<TcpStream>> {
    let ws = accept(stream?)?;
    Ok(ws)
}

fn handle_command(
    command: WsCommand,
    shared_data_inst: &SharedData,
    local_data: &mut LocalData,
) -> Option<WsReply> {
    match command {
        WsCommand::Reload => {
            shared_data_inst
                .signal_reload
                .store(true, Ordering::Relaxed);
            Some(WsReply::Reload {
                result: WsResult::Ok,
            })
        }
        WsCommand::GetCaptureRate => {
            let capstat = shared_data_inst.capture_status.lock().unwrap();
            Some(WsReply::GetCaptureRate {
                result: WsResult::Ok,
                value: capstat.measured_samplerate,
            })
        }
        WsCommand::GetSignalRange => {
            let capstat = shared_data_inst.capture_status.lock().unwrap();
            Some(WsReply::GetSignalRange {
                result: WsResult::Ok,
                value: capstat.signal_range,
            })
        }
        WsCommand::GetCaptureSignalRms => {
            let values = capture_signal_rms(shared_data_inst);
            Some(WsReply::GetCaptureSignalRms {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetCaptureSignalRmsSince(secs) => {
            let values = capture_signal_rms_since(shared_data_inst, secs);
            Some(WsReply::GetCaptureSignalRmsSince {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetCaptureSignalRmsSinceLast => {
            let values = capture_signal_rms_since_last(shared_data_inst, local_data);
            Some(WsReply::GetCaptureSignalRmsSinceLast {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetPlaybackSignalRms => {
            let values = playback_signal_rms(shared_data_inst);
            Some(WsReply::GetPlaybackSignalRms {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetPlaybackSignalRmsSince(secs) => {
            let values = playback_signal_rms_since(shared_data_inst, secs);
            Some(WsReply::GetPlaybackSignalRmsSince {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetPlaybackSignalRmsSinceLast => {
            let values = playback_signal_rms_since_last(shared_data_inst, local_data);
            Some(WsReply::GetPlaybackSignalRmsSinceLast {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetCaptureSignalPeak => {
            let values = capture_signal_peak(shared_data_inst);
            Some(WsReply::GetCaptureSignalPeak {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetCaptureSignalPeakSince(secs) => {
            let values = capture_signal_peak_since(shared_data_inst, secs);
            Some(WsReply::GetCaptureSignalPeakSince {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetCaptureSignalPeakSinceLast => {
            let values = capture_signal_peak_since_last(shared_data_inst, local_data);
            Some(WsReply::GetCaptureSignalPeakSinceLast {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetPlaybackSignalPeak => {
            let values = playback_signal_peak(shared_data_inst);
            Some(WsReply::GetPlaybackSignalPeak {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetPlaybackSignalPeakSince(secs) => {
            let values = playback_signal_peak_since(shared_data_inst, secs);
            Some(WsReply::GetPlaybackSignalPeakSince {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetPlaybackSignalPeakSinceLast => {
            let values = playback_signal_peak_since_last(shared_data_inst, local_data);
            Some(WsReply::GetPlaybackSignalPeakSinceLast {
                result: WsResult::Ok,
                value: values,
            })
        }
        WsCommand::GetSignalLevels => {
            let levels = AllLevels {
                playback_rms: playback_signal_rms(shared_data_inst),
                playback_peak: playback_signal_peak(shared_data_inst),
                capture_rms: capture_signal_rms(shared_data_inst),
                capture_peak: capture_signal_peak(shared_data_inst),
            };
            let result = WsReply::GetSignalLevels {
                result: WsResult::Ok,
                value: levels,
            };
            Some(result)
        }
        WsCommand::GetSignalLevelsSince(secs) => {
            let levels = AllLevels {
                playback_rms: playback_signal_rms_since(shared_data_inst, secs),
                playback_peak: playback_signal_peak_since(shared_data_inst, secs),
                capture_rms: capture_signal_rms_since(shared_data_inst, secs),
                capture_peak: capture_signal_peak_since(shared_data_inst, secs),
            };
            let result = WsReply::GetSignalLevelsSince {
                result: WsResult::Ok,
                value: levels,
            };
            Some(result)
        }
        WsCommand::GetSignalLevelsSinceLast => {
            let levels = AllLevels {
                playback_rms: playback_signal_rms_since_last(shared_data_inst, local_data),
                playback_peak: playback_signal_peak_since_last(shared_data_inst, local_data),
                capture_rms: capture_signal_rms_since_last(shared_data_inst, local_data),
                capture_peak: capture_signal_peak_since_last(shared_data_inst, local_data),
            };
            let result = WsReply::GetSignalLevelsSinceLast {
                result: WsResult::Ok,
                value: levels,
            };
            Some(result)
        }
        WsCommand::GetSignalPeaksSinceStart => {
            let levels = PbCapLevels {
                playback: playback_signal_global_peak(shared_data_inst),
                capture: capture_signal_global_peak(shared_data_inst),
            };
            let result = WsReply::GetSignalPeaksSinceStart {
                result: WsResult::Ok,
                value: levels,
            };
            Some(result)
        }
        WsCommand::ResetSignalPeaksSinceStart => {
            reset_playback_signal_global_peak(shared_data_inst);
            reset_capture_signal_global_peak(shared_data_inst);
            let result = WsReply::ResetSignalPeaksSinceStart {
                result: WsResult::Ok,
            };
            Some(result)
        }
        WsCommand::GetVersion => Some(WsReply::GetVersion {
            result: WsResult::Ok,
            value: crate_version!().to_string(),
        }),
        WsCommand::GetState => {
            let capstat = shared_data_inst.capture_status.lock().unwrap();
            Some(WsReply::GetState {
                result: WsResult::Ok,
                value: capstat.state,
            })
        }
        WsCommand::GetStopReason => {
            let stat = shared_data_inst.status.lock().unwrap();
            let value = stat.stop_reason.clone();
            Some(WsReply::GetStopReason {
                result: WsResult::Ok,
                value,
            })
        }
        WsCommand::GetRateAdjust => {
            let capstat = shared_data_inst.capture_status.lock().unwrap();
            Some(WsReply::GetRateAdjust {
                result: WsResult::Ok,
                value: capstat.rate_adjust,
            })
        }
        WsCommand::GetClippedSamples => {
            let pbstat = shared_data_inst.playback_status.lock().unwrap();
            Some(WsReply::GetClippedSamples {
                result: WsResult::Ok,
                value: pbstat.clipped_samples,
            })
        }
        WsCommand::ResetClippedSamples => {
            let mut pbstat = shared_data_inst.playback_status.lock().unwrap();
            pbstat.clipped_samples = 0;
            Some(WsReply::ResetClippedSamples {
                result: WsResult::Ok,
            })
        }
        WsCommand::GetBufferLevel => {
            let pbstat = shared_data_inst.playback_status.lock().unwrap();
            Some(WsReply::GetBufferLevel {
                result: WsResult::Ok,
                value: pbstat.buffer_level,
            })
        }
        WsCommand::GetUpdateInterval => {
            let capstat = shared_data_inst.capture_status.lock().unwrap();
            Some(WsReply::GetUpdateInterval {
                result: WsResult::Ok,
                value: capstat.update_interval,
            })
        }
        WsCommand::SetUpdateInterval(nbr) => {
            {
                let mut captstat = shared_data_inst.capture_status.lock().unwrap();
                let mut playstat = shared_data_inst.playback_status.lock().unwrap();
                captstat.update_interval = nbr;
                playstat.update_interval = nbr;
            }
            Some(WsReply::SetUpdateInterval {
                result: WsResult::Ok,
            })
        }
        WsCommand::GetVolume => {
            let procstat = shared_data_inst.processing_status.lock().unwrap();
            Some(WsReply::GetVolume {
                result: WsResult::Ok,
                value: procstat.target_volume[0],
            })
        }
        WsCommand::SetVolume(nbr) => {
            let new_vol = clamped_volume(nbr);
            let mut procstat = shared_data_inst.processing_status.lock().unwrap();
            procstat.target_volume[0] = new_vol;
            Some(WsReply::SetVolume {
                result: WsResult::Ok,
            })
        }
        WsCommand::AdjustVolume(nbr) => {
            let mut procstat = shared_data_inst.processing_status.lock().unwrap();
            let mut tempvol = procstat.target_volume[0];
            tempvol += nbr;
            tempvol = clamped_volume(tempvol);
            procstat.target_volume[0] = tempvol;
            Some(WsReply::AdjustVolume {
                result: WsResult::Ok,
                value: tempvol,
            })
        }
        WsCommand::GetMute => {
            let procstat = shared_data_inst.processing_status.lock().unwrap();
            Some(WsReply::GetMute {
                result: WsResult::Ok,
                value: procstat.mute[0],
            })
        }
        WsCommand::SetMute(mute) => {
            let mut procstat = shared_data_inst.processing_status.lock().unwrap();
            procstat.mute[0] = mute;
            Some(WsReply::SetMute {
                result: WsResult::Ok,
            })
        }
        WsCommand::ToggleMute => {
            let mut procstat = shared_data_inst.processing_status.lock().unwrap();
            procstat.mute[0] = !procstat.mute[0];
            Some(WsReply::ToggleMute {
                result: WsResult::Ok,
                value: procstat.mute[0],
            })
        }
        WsCommand::GetFaderVolume(ctrl) => {
            if ctrl > 4 {
                return Some(WsReply::GetFaderVolume {
                    result: WsResult::Error,
                    value: 0.0,
                    control: ctrl,
                });
            }
            let procstat = shared_data_inst.processing_status.lock().unwrap();
            Some(WsReply::GetFaderVolume {
                result: WsResult::Ok,
                value: procstat.target_volume[ctrl],
                control: ctrl,
            })
        }
        WsCommand::SetFaderVolume(ctrl, nbr) => {
            if ctrl > 4 {
                return Some(WsReply::SetFaderVolume {
                    result: WsResult::Error,
                    control: ctrl,
                });
            }
            let new_vol = clamped_volume(nbr);
            let mut procstat = shared_data_inst.processing_status.lock().unwrap();
            procstat.target_volume[ctrl] = new_vol;
            Some(WsReply::SetFaderVolume {
                result: WsResult::Ok,
                control: ctrl,
            })
        }
        WsCommand::SetFaderExternalVolume(ctrl, nbr) => {
            if ctrl > 4 {
                return Some(WsReply::SetFaderExternalVolume {
                    result: WsResult::Error,
                    control: ctrl,
                });
            }
            let new_vol = clamped_volume(nbr);
            let mut procstat = shared_data_inst.processing_status.lock().unwrap();
            procstat.target_volume[ctrl] = new_vol;
            procstat.current_volume[ctrl] = new_vol;
            Some(WsReply::SetFaderExternalVolume {
                result: WsResult::Ok,
                control: ctrl,
            })
        }
        WsCommand::AdjustFaderVolume(ctrl, nbr) => {
            if ctrl > 4 {
                return Some(WsReply::AdjustFaderVolume {
                    result: WsResult::Error,
                    value: nbr,
                    control: ctrl,
                });
            }
            let mut procstat = shared_data_inst.processing_status.lock().unwrap();
            let mut tempvol = procstat.target_volume[ctrl];
            tempvol += nbr;
            tempvol = clamped_volume(tempvol);
            procstat.target_volume[ctrl] = tempvol;
            Some(WsReply::AdjustFaderVolume {
                result: WsResult::Ok,
                value: tempvol,
                control: ctrl,
            })
        }
        WsCommand::GetFaderMute(ctrl) => {
            if ctrl > 4 {
                return Some(WsReply::GetFaderMute {
                    result: WsResult::Error,
                    value: false,
                    control: ctrl,
                });
            }
            let procstat = shared_data_inst.processing_status.lock().unwrap();
            Some(WsReply::GetFaderMute {
                result: WsResult::Ok,
                value: procstat.mute[ctrl],
                control: ctrl,
            })
        }
        WsCommand::SetFaderMute(ctrl, mute) => {
            if ctrl > 4 {
                return Some(WsReply::SetFaderMute {
                    result: WsResult::Error,
                    control: ctrl,
                });
            }
            let mut procstat = shared_data_inst.processing_status.lock().unwrap();
            procstat.mute[ctrl] = mute;
            Some(WsReply::SetFaderMute {
                result: WsResult::Ok,
                control: ctrl,
            })
        }
        WsCommand::ToggleFaderMute(ctrl) => {
            if ctrl > 4 {
                return Some(WsReply::ToggleFaderMute {
                    result: WsResult::Error,
                    value: false,
                    control: ctrl,
                });
            }
            let mut procstat = shared_data_inst.processing_status.lock().unwrap();
            procstat.mute[ctrl] = !procstat.mute[ctrl];
            Some(WsReply::ToggleFaderMute {
                result: WsResult::Ok,
                value: procstat.mute[ctrl],
                control: ctrl,
            })
        }
        WsCommand::GetConfig => Some(WsReply::GetConfig {
            result: WsResult::Ok,
            value: serde_yaml::to_string(&*shared_data_inst.active_config.lock().unwrap()).unwrap(),
        }),
        WsCommand::GetConfigTitle => {
            let optional_config = shared_data_inst.active_config.lock().unwrap();
            let value = if let Some(config) = &*optional_config {
                config.title.clone().unwrap_or_default()
            } else {
                String::new()
            };
            Some(WsReply::GetConfigTitle {
                result: WsResult::Ok,
                value,
            })
        }
        WsCommand::GetConfigDescription => {
            let optional_config = shared_data_inst.active_config.lock().unwrap();
            let value = if let Some(config) = &*optional_config {
                config.description.clone().unwrap_or_default()
            } else {
                String::new()
            };
            Some(WsReply::GetConfigDescription {
                result: WsResult::Ok,
                value,
            })
        }
        WsCommand::GetPreviousConfig => Some(WsReply::GetPreviousConfig {
            result: WsResult::Ok,
            value: serde_yaml::to_string(&*shared_data_inst.previous_config.lock().unwrap())
                .unwrap(),
        }),
        WsCommand::GetConfigJson => Some(WsReply::GetConfigJson {
            result: WsResult::Ok,
            value: serde_json::to_string(&*shared_data_inst.active_config.lock().unwrap()).unwrap(),
        }),
        WsCommand::GetConfigName => Some(WsReply::GetConfigName {
            result: WsResult::Ok,
            value: shared_data_inst
                .active_config_path
                .lock()
                .unwrap()
                .as_ref()
                .unwrap_or(&"NONE".to_string())
                .to_string(),
        }),
        WsCommand::SetConfigName(path) => match config::load_validate_config(&path) {
            Ok(_) => {
                *shared_data_inst.active_config_path.lock().unwrap() = Some(path.clone());
                Some(WsReply::SetConfigName {
                    result: WsResult::Ok,
                })
            }
            Err(error) => {
                error!("Error setting config name: {}", error);
                Some(WsReply::SetConfigName {
                    result: WsResult::Error,
                })
            }
        },
        WsCommand::SetConfig(config_yml) => {
            match serde_yaml::from_str::<config::Configuration>(&config_yml) {
                Ok(mut conf) => match config::validate_config(&mut conf, None) {
                    Ok(()) => {
                        *shared_data_inst.new_config.lock().unwrap() = Some(conf);
                        shared_data_inst
                            .signal_reload
                            .store(true, Ordering::Relaxed);
                        Some(WsReply::SetConfig {
                            result: WsResult::Ok,
                        })
                    }
                    Err(error) => {
                        error!("Error setting config: {}", error);
                        Some(WsReply::SetConfig {
                            result: WsResult::Error,
                        })
                    }
                },
                Err(error) => {
                    error!("Config error: {}", error);
                    Some(WsReply::SetConfig {
                        result: WsResult::Error,
                    })
                }
            }
        }
        WsCommand::SetConfigJson(config_json) => {
            match serde_json::from_str::<config::Configuration>(&config_json) {
                Ok(mut conf) => match config::validate_config(&mut conf, None) {
                    Ok(()) => {
                        *shared_data_inst.new_config.lock().unwrap() = Some(conf);
                        shared_data_inst
                            .signal_reload
                            .store(true, Ordering::Relaxed);
                        Some(WsReply::SetConfigJson {
                            result: WsResult::Ok,
                        })
                    }
                    Err(error) => {
                        error!("Error setting config: {}", error);
                        Some(WsReply::SetConfigJson {
                            result: WsResult::Error,
                        })
                    }
                },
                Err(error) => {
                    error!("Config error: {}", error);
                    Some(WsReply::SetConfigJson {
                        result: WsResult::Error,
                    })
                }
            }
        }
        WsCommand::ReadConfig(config_yml) => {
            match serde_yaml::from_str::<config::Configuration>(&config_yml) {
                Ok(conf) => Some(WsReply::ReadConfig {
                    result: WsResult::Ok,
                    value: serde_yaml::to_string(&conf).unwrap(),
                }),
                Err(error) => {
                    error!("Error reading config: {}", error);
                    Some(WsReply::ReadConfig {
                        result: WsResult::Error,
                        value: error.to_string(),
                    })
                }
            }
        }
        WsCommand::ReadConfigFile(path) => match config::load_config(&path) {
            Ok(conf) => Some(WsReply::ReadConfigFile {
                result: WsResult::Ok,
                value: serde_yaml::to_string(&conf).unwrap(),
            }),
            Err(error) => {
                error!("Error reading config file: {}", error);
                Some(WsReply::ReadConfigFile {
                    result: WsResult::Error,
                    value: error.to_string(),
                })
            }
        },
        WsCommand::ValidateConfig(config_yml) => {
            match serde_yaml::from_str::<config::Configuration>(&config_yml) {
                Ok(mut conf) => match config::validate_config(&mut conf, None) {
                    Ok(()) => Some(WsReply::ValidateConfig {
                        result: WsResult::Ok,
                        value: serde_yaml::to_string(&conf).unwrap(),
                    }),
                    Err(error) => {
                        error!("Config error: {}", error);
                        Some(WsReply::ValidateConfig {
                            result: WsResult::Error,
                            value: error.to_string(),
                        })
                    }
                },
                Err(error) => {
                    error!("Config error: {}", error);
                    Some(WsReply::ValidateConfig {
                        result: WsResult::Error,
                        value: error.to_string(),
                    })
                }
            }
        }
        WsCommand::Stop => {
            *shared_data_inst.new_config.lock().unwrap() = None;
            shared_data_inst
                .signal_exit
                .store(ExitRequest::STOP, Ordering::Relaxed);
            Some(WsReply::Stop {
                result: WsResult::Ok,
            })
        }
        WsCommand::Exit => {
            shared_data_inst
                .signal_exit
                .store(ExitRequest::EXIT, Ordering::Relaxed);
            Some(WsReply::Exit {
                result: WsResult::Ok,
            })
        }
        WsCommand::GetSupportedDeviceTypes => {
            let devs = list_supported_devices();
            Some(WsReply::GetSupportedDeviceTypes {
                result: WsResult::Ok,
                value: devs,
            })
        }
        WsCommand::None => None,
    }
}

fn clamped_volume(vol: f32) -> f32 {
    let mut new_vol = vol;
    // Clamp to -150 .. 50 dB, probably larger than needed..
    if new_vol < -150.0 {
        new_vol = -150.0;
        warn!("Clamped volume at -150 dB")
    } else if new_vol > 50.0 {
        new_vol = 50.0;
        warn!("Clamped volume at +50 dB")
    }
    new_vol
}

fn playback_signal_peak_since(shared_data: &SharedData, time: f32) -> Vec<f32> {
    let time_instant = Instant::now() - Duration::from_secs_f32(time);
    let res = shared_data
        .playback_status
        .lock()
        .unwrap()
        .signal_peak
        .max_since(time_instant);
    match res {
        Some(mut record) => {
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn playback_signal_rms_since(shared_data: &SharedData, time: f32) -> Vec<f32> {
    let time_instant = Instant::now() - Duration::from_secs_f32(time);
    let res = shared_data
        .playback_status
        .lock()
        .unwrap()
        .signal_rms
        .average_sqrt_since(time_instant);
    match res {
        Some(mut record) => {
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn capture_signal_peak_since(shared_data: &SharedData, time: f32) -> Vec<f32> {
    let time_instant = Instant::now() - Duration::from_secs_f32(time);
    let res = shared_data
        .capture_status
        .lock()
        .unwrap()
        .signal_peak
        .max_since(time_instant);
    match res {
        Some(mut record) => {
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn capture_signal_rms_since(shared_data: &SharedData, time: f32) -> Vec<f32> {
    let time_instant = Instant::now() - Duration::from_secs_f32(time);
    let res = shared_data
        .capture_status
        .lock()
        .unwrap()
        .signal_rms
        .average_sqrt_since(time_instant);
    match res {
        Some(mut record) => {
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn playback_signal_peak_since_last(
    shared_data: &SharedData,
    local_data: &mut LocalData,
) -> Vec<f32> {
    let res = shared_data
        .playback_status
        .lock()
        .unwrap()
        .signal_peak
        .max_since(local_data.last_pb_peak_time);
    match res {
        Some(mut record) => {
            local_data.last_pb_peak_time = record.time;
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn playback_signal_rms_since_last(
    shared_data: &SharedData,
    local_data: &mut LocalData,
) -> Vec<f32> {
    let res = shared_data
        .playback_status
        .lock()
        .unwrap()
        .signal_rms
        .average_sqrt_since(local_data.last_pb_rms_time);
    match res {
        Some(mut record) => {
            local_data.last_pb_rms_time = record.time;
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn capture_signal_peak_since_last(
    shared_data: &SharedData,
    local_data: &mut LocalData,
) -> Vec<f32> {
    let res = shared_data
        .capture_status
        .lock()
        .unwrap()
        .signal_peak
        .max_since(local_data.last_cap_peak_time);
    match res {
        Some(mut record) => {
            local_data.last_cap_peak_time = record.time;
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn capture_signal_rms_since_last(shared_data: &SharedData, local_data: &mut LocalData) -> Vec<f32> {
    let res = shared_data
        .capture_status
        .lock()
        .unwrap()
        .signal_rms
        .average_sqrt_since(local_data.last_cap_rms_time);
    match res {
        Some(mut record) => {
            local_data.last_cap_rms_time = record.time;
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn playback_signal_peak(shared_data: &SharedData) -> Vec<f32> {
    let res = shared_data
        .playback_status
        .lock()
        .unwrap()
        .signal_peak
        .last();
    match res {
        Some(mut record) => {
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn playback_signal_global_peak(shared_data: &SharedData) -> Vec<f32> {
    shared_data
        .playback_status
        .lock()
        .unwrap()
        .signal_peak
        .global_max()
}

fn reset_playback_signal_global_peak(shared_data: &SharedData) {
    shared_data
        .playback_status
        .lock()
        .unwrap()
        .signal_peak
        .reset_global_max();
}

fn playback_signal_rms(shared_data: &SharedData) -> Vec<f32> {
    let res = shared_data
        .playback_status
        .lock()
        .unwrap()
        .signal_rms
        .last_sqrt();
    match res {
        Some(mut record) => {
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn capture_signal_peak(shared_data: &SharedData) -> Vec<f32> {
    let res = shared_data
        .capture_status
        .lock()
        .unwrap()
        .signal_peak
        .last();
    match res {
        Some(mut record) => {
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

fn capture_signal_global_peak(shared_data: &SharedData) -> Vec<f32> {
    shared_data
        .capture_status
        .lock()
        .unwrap()
        .signal_peak
        .global_max()
}

fn reset_capture_signal_global_peak(shared_data: &SharedData) {
    shared_data
        .capture_status
        .lock()
        .unwrap()
        .signal_peak
        .reset_global_max();
}

fn capture_signal_rms(shared_data: &SharedData) -> Vec<f32> {
    let res = shared_data
        .capture_status
        .lock()
        .unwrap()
        .signal_rms
        .last_sqrt();
    match res {
        Some(mut record) => {
            linear_to_db(&mut record.values);
            record.values
        }
        None => vec![],
    }
}

#[cfg(test)]
mod tests {
    use crate::socketserver::{parse_command, WsCommand};
    use tungstenite::Message;

    #[test]
    fn parse_commands() {
        let cmd = Message::text("\"Reload\"");
        let res = parse_command(cmd).unwrap();
        assert_eq!(res, WsCommand::Reload);
        let cmd = Message::text("asdfasdf");
        let res = parse_command(cmd);
        assert!(res.is_err());
        let cmd = Message::text("");
        let res = parse_command(cmd);
        assert!(res.is_err());
        let cmd = Message::text("{\"SetConfigName\": \"somefile\"}");
        let res = parse_command(cmd).unwrap();
        assert_eq!(res, WsCommand::SetConfigName("somefile".to_string()));
    }
}
