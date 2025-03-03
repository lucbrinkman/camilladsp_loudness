use crate::basicfilters::Gain;
use crate::biquad;
use crate::config;
use crate::filters::Filter;
use std::sync::Arc;

use crate::PrcFmt;
use crate::ProcessingParameters;
use crate::Res;

const LOW_SHELF_GAIN_FACTOR: f64 = 0.52;
const LOUDNESS_EFFECT_STRENGTH: f64 = 1.0; // At 1.0, it compensated exactly for the Fletcher-Munson curves.

pub struct Loudness {
    pub name: String,
    current_volume: PrcFmt,
    processing_params: Arc<ProcessingParameters>,
    reference_level: f32,
    high_biquad: biquad::Biquad,
    low_biquad: biquad::Biquad,
    fader: usize,
    active: bool,
    gain: Option<Gain>,
    peaking_biquad_1: biquad::Biquad, // Rename to peaking_biquad_1
    peaking_biquad_2: biquad::Biquad, // Add peaking_biquad_2
    peaking_biquad_3: biquad::Biquad, // Add peaking_biquad_3
}

fn calc_loudness_gain(level: f32, reference: f32) -> f32 {
    let loudness_gain = reference - level; // In this new system we just want to know the absolute dB change to correct for.
    loudness_gain.clamp(0.0, 40.0) // Clamped this to max 40dB for safety. This would lead to a max bass boost 
}

impl Loudness {
    fn create_highshelf_conf(loudness_gain: PrcFmt) -> config::BiquadParameters {
        config::BiquadParameters::Highshelf(config::ShelfSteepness::Q {
            freq: 10620.0,
            q: 1.38,
            gain: 0.1456 * LOUDNESS_EFFECT_STRENGTH * loudness_gain,
        })
    }

    fn create_lowshelf_conf(loudness_gain: PrcFmt) -> config::BiquadParameters {
        config::BiquadParameters::Lowshelf(config::ShelfSteepness::Slope {
            freq: 120.0,
            slope: 6.0,
            gain: LOW_SHELF_GAIN_FACTOR * LOUDNESS_EFFECT_STRENGTH * loudness_gain,
        })
    }

    fn create_peaking_conf_1(loudness_gain: PrcFmt) -> config::BiquadParameters {
        config::BiquadParameters::Peaking(config::PeakingWidth::Q {
            freq: 2000.0,
            q: 0.6,
            gain: -0.0312 * LOUDNESS_EFFECT_STRENGTH * loudness_gain,
        })
    }

    fn create_peaking_conf_2(loudness_gain: PrcFmt) -> config::BiquadParameters {
        config::BiquadParameters::Peaking(config::PeakingWidth::Q {
            freq: 4000.0,
            q: 0.8,
            gain: -0.01404 * LOUDNESS_EFFECT_STRENGTH * loudness_gain,
        })
    }

    fn create_peaking_conf_3(loudness_gain: PrcFmt) -> config::BiquadParameters {
        config::BiquadParameters::Peaking(config::PeakingWidth::Q {
            freq: 8000.0,
            q: 2.13,
            gain: 0.0364 * LOUDNESS_EFFECT_STRENGTH * loudness_gain,
        })
    }

    pub fn from_config(
        name: &str,
        conf: config::LoudnessParameters,
        samplerate: usize,
        processing_params: Arc<ProcessingParameters>,
    ) -> Self {
        info!("Create loudness filter");
        let fader = conf.fader();
        let current_volume = processing_params.target_volume(fader);
        let loudness_gain = calc_loudness_gain(current_volume, conf.reference_level) as PrcFmt;
        let active = loudness_gain > 0.01;
        // let high_boost = (loudness_gain * conf.high_boost()) as PrcFmt;
        // let low_boost = (loudness_gain * conf.low_boost()) as PrcFmt;
        let highshelf_conf = Loudness::create_highshelf_conf(loudness_gain);
        let lowshelf_conf = Loudness::create_lowshelf_conf(loudness_gain);
        let peaking_conf_1 = Loudness::create_peaking_conf_1(loudness_gain);
        let peaking_conf_2 = Loudness::create_peaking_conf_2(loudness_gain);
        let peaking_conf_3 = Loudness::create_peaking_conf_3(loudness_gain);
        let gain = if conf.attenuate_mid() {
            let max_gain = LOW_SHELF_GAIN_FACTOR * loudness_gain;
            let gain_params = config::GainParameters {
                gain: -max_gain,
                inverted: None,
                mute: None,
                scale: None,
            };
            Some(Gain::from_config("midgain", gain_params))
        } else {
            None
        };

        let high_biquad_coeffs =
            biquad::BiquadCoefficients::from_config(samplerate, highshelf_conf);
        let low_biquad_coeffs = biquad::BiquadCoefficients::from_config(samplerate, lowshelf_conf);
        let peaking_biquad_coeffs_1 = biquad::BiquadCoefficients::from_config(samplerate, peaking_conf_1);
        let peaking_biquad_coeffs_2 = biquad::BiquadCoefficients::from_config(samplerate, peaking_conf_2);
        let peaking_biquad_coeffs_3 = biquad::BiquadCoefficients::from_config(samplerate, peaking_conf_3);
        let high_biquad = biquad::Biquad::new("highshelf", samplerate, high_biquad_coeffs);
        let low_biquad = biquad::Biquad::new("lowshelf", samplerate, low_biquad_coeffs);
        let peaking_biquad_1 = biquad::Biquad::new("peaking_1", samplerate, peaking_biquad_coeffs_1);
        let peaking_biquad_2 = biquad::Biquad::new("peaking_2", samplerate, peaking_biquad_coeffs_2);
        let peaking_biquad_3 = biquad::Biquad::new("peaking_3", samplerate, peaking_biquad_coeffs_3);

        Loudness {
            name: name.to_string(),
            current_volume: current_volume as PrcFmt,
            reference_level: conf.reference_level,
            // high_boost: conf.high_boost(),
            // low_boost: conf.low_boost(),
            high_biquad,
            low_biquad,
            peaking_biquad_1,
            peaking_biquad_2,
            peaking_biquad_3,
            processing_params,
            fader,
            active,
            gain,
        }
    }
}

impl Filter for Loudness {
    fn name(&self) -> &str {
        &self.name
    }

    fn process_waveform(&mut self, waveform: &mut [PrcFmt]) -> Res<()> {
        let shared_vol = self.processing_params.current_volume(self.fader);

        // Volume setting changed
        if (shared_vol - self.current_volume as f32).abs() > 0.01 {
            self.current_volume = shared_vol as PrcFmt;
            let loudness_gain = calc_loudness_gain(self.current_volume as f32, self.reference_level) as PrcFmt;
            // let high_boost = (loudness_gain * self.high_boost) as PrcFmt;
            // let low_boost = (loudness_gain * self.low_boost) as PrcFmt;
            self.active = loudness_gain > 0.001;
            debug!(
                "Updating loudness biquads, relative boost {}%",
                100.0 * loudness_gain
            );
            let highshelf_conf = Loudness::create_highshelf_conf(loudness_gain);
            self.high_biquad.update_parameters(config::Filter::Biquad {
                parameters: highshelf_conf,
                description: None,
            });
            let lowshelf_conf = Loudness::create_lowshelf_conf(loudness_gain);
            self.low_biquad.update_parameters(config::Filter::Biquad {
                parameters: lowshelf_conf,
                description: None,
            });
            let peaking_conf_1 = Loudness::create_peaking_conf_1(loudness_gain);
            self.peaking_biquad_1.update_parameters(config::Filter::Biquad {
                parameters: peaking_conf_1,
                description: None,
            });
            let peaking_conf_2 = Loudness::create_peaking_conf_2(loudness_gain);
            self.peaking_biquad_2.update_parameters(config::Filter::Biquad {
                parameters: peaking_conf_2,
                description: None,
            });
            let peaking_conf_3 = Loudness::create_peaking_conf_3(loudness_gain);
            self.peaking_biquad_3.update_parameters(config::Filter::Biquad {
                parameters: peaking_conf_3,
                description: None,
            });
            if let Some(gain) = &mut self.gain {
                let max_gain = LOW_SHELF_GAIN_FACTOR * loudness_gain;
                let gain_params = config::GainParameters {
                    gain: -max_gain,
                    inverted: None,
                    mute: None,
                    scale: None,
                };
                gain.update_parameters(config::Filter::Gain {
                    description: None,
                    parameters: gain_params,
                });
            }
        }
        if self.active {
            trace!("Applying loudness biquads");
            self.high_biquad.process_waveform(waveform).unwrap();
            self.low_biquad.process_waveform(waveform).unwrap();
            self.peaking_biquad_1.process_waveform(waveform).unwrap();
            self.peaking_biquad_2.process_waveform(waveform).unwrap();
            self.peaking_biquad_3.process_waveform(waveform).unwrap();
            if let Some(gain) = &mut self.gain {
                gain.process_waveform(waveform).unwrap();
            }
        }
        Ok(())
    }

    fn update_parameters(&mut self, conf: config::Filter) {
        if let config::Filter::Loudness {
            parameters: conf, ..
        } = conf
        {
            self.fader = conf.fader();
            let current_volume = self.processing_params.current_volume(self.fader);
            let loudness_gain = calc_loudness_gain(current_volume, conf.reference_level) as PrcFmt;
            // let high_boost = (loudness_gain * conf.high_boost()) as PrcFmt;
            // let low_boost = (loudness_gain * conf.low_boost()) as PrcFmt;
            self.active = loudness_gain > 0.001;
            let highshelf_conf = Loudness::create_highshelf_conf(loudness_gain);
            self.high_biquad.update_parameters(config::Filter::Biquad {
                parameters: highshelf_conf,
                description: None,
            });
            let lowshelf_conf = Loudness::create_lowshelf_conf(loudness_gain);
            self.low_biquad.update_parameters(config::Filter::Biquad {
                parameters: lowshelf_conf,
                description: None,
            });
            let peaking_conf_1 = Loudness::create_peaking_conf_1(loudness_gain);
            self.peaking_biquad_1.update_parameters(config::Filter::Biquad {
                parameters: peaking_conf_1,
                description: None,
            });
            let peaking_conf_2 = Loudness::create_peaking_conf_2(loudness_gain);
            self.peaking_biquad_2.update_parameters(config::Filter::Biquad {
                parameters: peaking_conf_2,
                description: None,
            });
            let peaking_conf_3 = Loudness::create_peaking_conf_3(loudness_gain);
            self.peaking_biquad_3.update_parameters(config::Filter::Biquad {
                parameters: peaking_conf_3,
                description: None,
            });
            if conf.attenuate_mid() {
                let max_gain = LOW_SHELF_GAIN_FACTOR * loudness_gain;
                let gain_params = config::GainParameters {
                    gain: -max_gain,
                    inverted: None,
                    mute: None,
                    scale: None,
                };
                if let Some(gain) = &mut self.gain {
                    gain.update_parameters(config::Filter::Gain {
                        description: None,
                        parameters: gain_params,
                    });
                } else {
                    self.gain = Some(Gain::from_config("midgain", gain_params))
                }
            } else {
                self.gain = None
            }

            self.reference_level = conf.reference_level;
            // self.high_boost = conf.high_boost();
            // self.low_boost = conf.low_boost();
        } else {
            // This should never happen unless there is a bug somewhere else
            panic!("Invalid config change!");
        }
    }
}

/// Validate a Loudness config.
pub fn validate_config(conf: &config::LoudnessParameters) -> Res<()> {
    if conf.reference_level > 0.0 {
        return Err(config::ConfigError::new("Reference level must be less than 0").into());
    } else if conf.reference_level < -100.0 {
        return Err(config::ConfigError::new("Reference level must be higher than -100").into());
    // } else if conf.high_boost() < 0.0 {
    //     return Err(config::ConfigError::new("High boost cannot be less than 0").into());
    // } else if conf.low_boost() < 0.0 {
    //     return Err(config::ConfigError::new("Low boost cannot be less than 0").into());
    // } else if conf.high_boost() > 20.0 {
    //     return Err(config::ConfigError::new("High boost cannot be larger than 20").into());
    // } else if conf.low_boost() > 20.0 {
    //     return Err(config::ConfigError::new("Low boost cannot be larger than 20").into());
    }
    Ok(())
}
