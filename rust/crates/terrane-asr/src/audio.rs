//! PCM conversion helpers — whisper.cpp expects 16 kHz mono f32.

use crate::AsrError;

/// The sample rate whisper.cpp requires.
pub const WHISPER_SAMPLE_RATE_HZ: u32 = 16_000;

/// Convert mono i16 PCM at `sample_rate_hz` to 16 kHz mono f32 for whisper.cpp.
pub fn pcm_i16_to_f32_mono_16k(pcm: &[i16], sample_rate_hz: u32) -> Result<Vec<f32>, AsrError> {
    if sample_rate_hz == 0 {
        return Err(AsrError::Transcribe("sample rate must be non-zero".into()));
    }
    if pcm.is_empty() {
        return Ok(Vec::new());
    }
    let floats = pcm.iter().map(|&sample| sample as f32 / 32768.0).collect::<Vec<_>>();
    if sample_rate_hz == WHISPER_SAMPLE_RATE_HZ {
        return Ok(floats);
    }
    resample_linear(&floats, sample_rate_hz, WHISPER_SAMPLE_RATE_HZ)
}

fn resample_linear(input: &[f32], from_hz: u32, to_hz: u32) -> Result<Vec<f32>, AsrError> {
    if from_hz == 0 || to_hz == 0 {
        return Err(AsrError::Transcribe("sample rate must be non-zero".into()));
    }
    if input.is_empty() {
        return Ok(Vec::new());
    }
    if from_hz == to_hz {
        return Ok(input.to_vec());
    }

    let ratio = f64::from(from_hz) / f64::from(to_hz);
    let out_len = ((input.len() as f64) / ratio).ceil() as usize;
    let mut out = Vec::with_capacity(out_len);
    for out_idx in 0..out_len {
        let src_pos = out_idx as f64 * ratio;
        let left = src_pos.floor() as usize;
        let right = (left + 1).min(input.len() - 1);
        let frac = (src_pos - left as f64) as f32;
        let sample = input[left] * (1.0 - frac) + input[right] * frac;
        out.push(sample);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_pcm_yields_empty_audio() {
        assert!(pcm_i16_to_f32_mono_16k(&[], 16_000).unwrap().is_empty());
    }

    #[test]
    fn identity_at_16khz_preserves_length() {
        let pcm = [0i16, i16::MAX, i16::MIN, 8_192];
        let out = pcm_i16_to_f32_mono_16k(&pcm, 16_000).unwrap();
        assert_eq!(out.len(), pcm.len());
        assert!((out[1] - 1.0).abs() < 1e-4);
        assert!((out[2] + 1.0).abs() < 1e-4);
    }

    #[test]
    fn downsampling_halves_length_for_32khz_input() {
        let pcm: Vec<i16> = (0..32).map(|i| i as i16 * 100).collect();
        let out = pcm_i16_to_f32_mono_16k(&pcm, 32_000).unwrap();
        assert_eq!(out.len(), 16);
    }
}