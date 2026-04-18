pub struct AudioProcessor {
    noise_gate: NoiseGate,
    agc: AutomaticGainControl,
    compressor: Compressor,
    capture_high_pass_filter: HighPassFilter,
    playback_high_pass_filter: HighPassFilter,
    #[allow(dead_code)]
    sample_rate: u32,
    #[allow(dead_code)]
    channels: u32,
}

impl AudioProcessor {
    pub fn new(sample_rate: u32, channels: u32) -> Self {
        Self {
            noise_gate: NoiseGate::new(),
            agc: AutomaticGainControl::new(),
            compressor: Compressor::new(),
            capture_high_pass_filter: HighPassFilter::new(sample_rate),
            playback_high_pass_filter: HighPassFilter::new(sample_rate),
            sample_rate,
            channels,
        }
    }

    pub fn process_capture(&mut self, samples: &[i16]) -> Vec<i16> {
        let mut result = samples.to_vec();

        result = self.capture_high_pass_filter.process(&result);
        result = self.noise_gate.process(&result);
        result = self.agc.process(&result);
        result = self.compressor.process(&result);

        result
    }

    pub fn process_playback(&mut self, samples: &[i16]) -> Vec<i16> {
        let mut result = samples.to_vec();
        result = self.playback_high_pass_filter.process(&result);
        result
    }

    pub fn encode(&self, samples: &[i16]) -> Vec<u8> {
        Self::encode_pcm_rle(samples)
    }

    pub fn decode(&self, data: &[u8]) -> Vec<i16> {
        Self::decode_pcm_rle(data)
    }

    pub fn encode_pcm_rle(samples: &[i16]) -> Vec<u8> {
        const OP_LITERAL: u8 = 0;
        const OP_RUN: u8 = 1;
        const RUN_THRESHOLD: usize = 4;

        let mut encoded = Vec::with_capacity(samples.len() * 2);
        let mut i = 0usize;

        while i < samples.len() {
            // Find run length for the current sample.
            let mut run_len = 1usize;
            while i + run_len < samples.len()
                && samples[i + run_len] == samples[i]
                && run_len < u16::MAX as usize
            {
                run_len += 1;
            }

            if run_len >= RUN_THRESHOLD {
                encoded.push(OP_RUN);
                encoded.extend_from_slice(&(run_len as u16).to_le_bytes());
                encoded.extend_from_slice(&samples[i].to_le_bytes());
                i += run_len;
                continue;
            }

            // Build a literal block until the next compressible run.
            let literal_start = i;
            let mut literal_len = 0usize;
            while i < samples.len() && literal_len < u16::MAX as usize {
                let mut next_run_len = 1usize;
                while i + next_run_len < samples.len()
                    && samples[i + next_run_len] == samples[i]
                    && next_run_len < u16::MAX as usize
                {
                    next_run_len += 1;
                }
                if next_run_len >= RUN_THRESHOLD {
                    break;
                }
                i += next_run_len;
                literal_len += next_run_len;
            }

            encoded.push(OP_LITERAL);
            encoded.extend_from_slice(&(literal_len as u16).to_le_bytes());
            for sample in &samples[literal_start..literal_start + literal_len] {
                encoded.extend_from_slice(&sample.to_le_bytes());
            }
        }

        encoded
    }

    pub fn decode_pcm_rle(data: &[u8]) -> Vec<i16> {
        const OP_LITERAL: u8 = 0;
        const OP_RUN: u8 = 1;

        let mut decoded = Vec::with_capacity(data.len() / 2);
        let mut i = 0;

        while i < data.len() {
            if i + 2 >= data.len() {
                break;
            }

            let op = data[i];
            let count = u16::from_le_bytes([data[i + 1], data[i + 2]]) as usize;
            i += 3;

            match op {
                OP_LITERAL => {
                    let bytes_needed = count.saturating_mul(2);
                    if i + bytes_needed > data.len() {
                        break;
                    }
                    for chunk in data[i..i + bytes_needed].chunks_exact(2) {
                        decoded.push(i16::from_le_bytes([chunk[0], chunk[1]]));
                    }
                    i += bytes_needed;
                }
                OP_RUN => {
                    if i + 1 >= data.len() {
                        break;
                    }
                    let sample = i16::from_le_bytes([data[i], data[i + 1]]);
                    for _ in 0..count {
                        decoded.push(sample);
                    }
                    i += 2;
                }
                _ => break,
            }
        }

        decoded
    }
}

pub struct NoiseGate {
    threshold: f32,
    #[allow(dead_code)]
    attack_coefficient: f32,
    release_coefficient: f32,
    envelope: f32,
    min_envelope: f32,
}

impl NoiseGate {
    pub fn new() -> Self {
        const SAMPLE_RATE: f32 = 48000.0;
        Self {
            threshold: 0.008,
            attack_coefficient: (-1.0 / (SAMPLE_RATE * 0.003)).exp(),
            release_coefficient: (-1.0 / (SAMPLE_RATE * 0.15)).exp(),
            envelope: 0.0,
            min_envelope: 0.001,
        }
    }

    pub fn process(&mut self, samples: &[i16]) -> Vec<i16> {
        let input_scale = 1.0 / i16::MAX as f32;
        let mut output = Vec::with_capacity(samples.len());

        for &sample in samples {
            let input = sample as f32 * input_scale;
            let abs_input = input.abs();

            if abs_input > self.threshold {
                self.envelope = self.envelope.max(abs_input).min(1.0);
            } else {
                self.envelope = (self.envelope * self.release_coefficient).max(self.min_envelope);
            }

            let gate = if self.envelope > self.threshold {
                1.0
            } else {
                0.0
            };
            let output_sample = (input * gate * i16::MAX as f32) as i16;
            output.push(output_sample);
        }

        output
    }
}

impl Default for NoiseGate {
    fn default() -> Self {
        Self::new()
    }
}

pub struct AutomaticGainControl {
    target_level: f32,
    max_gain: f32,
    min_gain: f32,
    attack_coefficient: f32,
    release_coefficient: f32,
    current_gain: f32,
    peak_hold: f32,
    hold_counter: usize,
}

impl AutomaticGainControl {
    pub fn new() -> Self {
        const SAMPLE_RATE: f32 = 48000.0;
        Self {
            target_level: 0.35,
            max_gain: 8.0,
            min_gain: 0.25,
            attack_coefficient: (-1.0 / (SAMPLE_RATE * 0.008)).exp(),
            release_coefficient: (-1.0 / (SAMPLE_RATE * 0.25)).exp(),
            current_gain: 1.0,
            peak_hold: 0.0,
            hold_counter: 0,
        }
    }

    pub fn process(&mut self, samples: &[i16]) -> Vec<i16> {
        let input_scale = 1.0 / i16::MAX as f32;
        let mut output = Vec::with_capacity(samples.len());
        let mut block_peak = 0.0f32;

        for &sample in samples {
            let input = sample as f32 * input_scale;
            let abs_input = input.abs();

            if abs_input > block_peak {
                block_peak = abs_input;
            }
        }

        if block_peak > self.peak_hold {
            self.peak_hold = block_peak;
            self.hold_counter = 48;
        } else if self.hold_counter > 0 {
            self.hold_counter -= 1;
        } else {
            self.peak_hold *= 0.9995;
        }

        let desired_gain = if self.peak_hold > 0.001 {
            (self.target_level / self.peak_hold).clamp(self.min_gain, self.max_gain)
        } else {
            self.min_gain
        };

        if desired_gain > self.current_gain {
            self.current_gain = self.current_gain * (1.0 - self.attack_coefficient)
                + desired_gain * self.attack_coefficient;
        } else {
            self.current_gain = self.current_gain * (1.0 - self.release_coefficient)
                + desired_gain * self.release_coefficient;
        }

        for &sample in samples {
            let input = sample as f32 * input_scale;
            let output_sample =
                (input * self.current_gain * i16::MAX as f32).clamp(-32768.0, 32767.0) as i16;
            output.push(output_sample);
        }

        output
    }
}

impl Default for AutomaticGainControl {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Compressor {
    threshold: f32,
    ratio: f32,
    attack_coefficient: f32,
    release_coefficient: f32,
    gain_reduction: f32,
    knee_width: f32,
}

impl Compressor {
    pub fn new() -> Self {
        const SAMPLE_RATE: f32 = 48000.0;
        Self {
            threshold: 0.5,
            ratio: 3.0,
            attack_coefficient: (-1.0 / (SAMPLE_RATE * 0.005)).exp(),
            release_coefficient: (-1.0 / (SAMPLE_RATE * 0.1)).exp(),
            gain_reduction: 0.0,
            knee_width: 0.1,
        }
    }

    pub fn process(&mut self, samples: &[i16]) -> Vec<i16> {
        let input_scale = 1.0 / i16::MAX as f32;
        let mut output = Vec::with_capacity(samples.len());

        for &sample in samples {
            let input = sample as f32 * input_scale;
            let abs_input = input.abs();

            let compressed = if abs_input > self.threshold - self.knee_width / 2.0 {
                let excess =
                    (abs_input - (self.threshold - self.knee_width / 2.0)) / self.knee_width;
                let compressed_input = self.threshold - self.knee_width / 2.0
                    + (excess.min(1.0))
                        * (self.knee_width / 2.0 + (abs_input - self.threshold) / self.ratio);
                compressed_input * input.signum()
            } else {
                input
            };

            let target_gr = if abs_input > self.threshold {
                1.0 - (abs_input - self.threshold) / (1.0 - self.threshold)
                    * (1.0 - 1.0 / self.ratio)
            } else {
                1.0
            };

            if target_gr < self.gain_reduction {
                self.gain_reduction = self.gain_reduction * (1.0 - self.attack_coefficient)
                    + target_gr * self.attack_coefficient;
            } else {
                self.gain_reduction = self.gain_reduction * (1.0 - self.release_coefficient)
                    + target_gr * self.release_coefficient;
            }

            let output_sample = (compressed * self.gain_reduction.clamp(0.1, 1.0) * i16::MAX as f32)
                .clamp(-32768.0, 32767.0) as i16;
            output.push(output_sample);
        }

        output
    }
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

pub struct HighPassFilter {
    alpha: f32,
    last_input: f32,
    last_output: f32,
}

impl HighPassFilter {
    pub fn new(sample_rate: u32) -> Self {
        let cutoff = 80.0;
        let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff);
        let dt = 1.0 / sample_rate as f32;
        let alpha = rc / (rc + dt);

        Self {
            alpha,
            last_input: 0.0,
            last_output: 0.0,
        }
    }

    pub fn process(&mut self, samples: &[i16]) -> Vec<i16> {
        let mut output = Vec::with_capacity(samples.len());

        for &sample in samples {
            let input = sample as f32;
            // First-order RC high-pass: y[n] = a * (y[n-1] + x[n] - x[n-1]).
            let filtered = self.alpha * (self.last_output + input - self.last_input);
            self.last_input = input;
            self.last_output = filtered;
            output.push(filtered.clamp(-32768.0, 32767.0) as i16);
        }

        output
    }
}

impl Default for HighPassFilter {
    fn default() -> Self {
        Self::new(48000)
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioProcessor, HighPassFilter};

    #[test]
    fn high_pass_filter_rejects_dc_after_warmup() {
        let mut filter = HighPassFilter::new(48_000);
        let input = vec![10_000i16; 1024];
        let output = filter.process(&input);
        let tail = output.last().copied().unwrap_or_default().abs();
        assert!(
            tail < 100,
            "expected strong DC attenuation in steady state, got tail sample {}",
            tail
        );
    }

    #[test]
    fn high_pass_filter_preserves_transients() {
        let mut filter = HighPassFilter::new(48_000);
        let mut input = vec![0i16; 16];
        input[0] = 12_000;
        let output = filter.process(&input);
        assert!(
            output.iter().any(|sample| sample.abs() > 1000),
            "expected impulse response to remain visible after filtering"
        );
    }

    #[test]
    fn rle_roundtrip_preserves_short_runs() {
        let codec = AudioProcessor::new(48_000, 1);
        let input = vec![100, 100, 200, 200, 200, 300];
        let encoded = codec.encode(&input);
        let decoded = codec.decode(&encoded);
        assert_eq!(decoded, input);
    }

    #[test]
    fn rle_roundtrip_preserves_samples_with_ff00_bytes() {
        let codec = AudioProcessor::new(48_000, 1);
        let input = vec![0x00FFi16, -257i16, 0x00FFi16, 1024i16, -1024i16];
        let encoded = codec.encode(&input);
        let decoded = codec.decode(&encoded);
        assert_eq!(decoded, input);
    }
}
