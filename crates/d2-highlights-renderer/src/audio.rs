use std::f32::consts::PI;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const BITS_PER_SAMPLE: u16 = 16;
const BPM: f32 = 138.0;

pub(crate) fn write_original_bgm(
    path: &Path,
    duration_seconds: f32,
    impact_cues: &[f32],
) -> std::io::Result<()> {
    let sample_count = (duration_seconds.max(0.1) * SAMPLE_RATE as f32).ceil() as u32;
    let bytes_per_sample = (BITS_PER_SAMPLE / 8) as u32;
    let data_size = sample_count * CHANNELS as u32 * bytes_per_sample;
    let mut writer = BufWriter::new(File::create(path)?);

    writer.write_all(b"RIFF")?;
    writer.write_all(&(36 + data_size).to_le_bytes())?;
    writer.write_all(b"WAVEfmt ")?;
    writer.write_all(&16_u32.to_le_bytes())?;
    writer.write_all(&1_u16.to_le_bytes())?;
    writer.write_all(&CHANNELS.to_le_bytes())?;
    writer.write_all(&SAMPLE_RATE.to_le_bytes())?;
    writer.write_all(&(SAMPLE_RATE * CHANNELS as u32 * bytes_per_sample).to_le_bytes())?;
    writer.write_all(&(CHANNELS * BITS_PER_SAMPLE / 8).to_le_bytes())?;
    writer.write_all(&BITS_PER_SAMPLE.to_le_bytes())?;
    writer.write_all(b"data")?;
    writer.write_all(&data_size.to_le_bytes())?;

    let beat = 60.0 / BPM;
    let eighth = beat / 2.0;
    let melody = [
        220.0_f32, 0.0, 261.626, 293.665, 0.0, 329.628, 293.665, 261.626, 220.0, 0.0, 293.665,
        329.628, 391.995, 329.628, 293.665, 261.626,
    ];
    let bass = [110.0_f32, 110.0, 82.407, 98.0];
    let mut noise_state = 0x5A17_91E3_u32;

    for sample_index in 0..sample_count {
        let time = sample_index as f32 / SAMPLE_RATE as f32;
        let eighth_index = (time / eighth).floor() as usize;
        let eighth_phase = time % eighth;
        let beat_index = (time / beat).floor() as usize;
        let beat_phase = time % beat;

        noise_state = noise_state
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        let noise = ((noise_state >> 8) as f32 / 16_777_215.0) * 2.0 - 1.0;

        let kick_frequency = 54.0 + 72.0 * (-beat_phase * 22.0).exp();
        let kick =
            (2.0 * PI * kick_frequency * beat_phase).sin() * (-beat_phase * 12.0).exp() * 0.42;

        let snare_phase = (time + beat * 0.5) % (beat * 2.0);
        let snare = if snare_phase < 0.18 {
            noise * (-snare_phase * 21.0).exp() * 0.15
        } else {
            0.0
        };

        let hat = noise * (-eighth_phase * 52.0).exp() * 0.055;

        let bass_frequency = bass[(beat_index / 2) % bass.len()];
        let bass_wave = (2.0 * PI * bass_frequency * time).sin() * (-beat_phase * 3.4).exp() * 0.15;

        let melody_frequency = melody[eighth_index % melody.len()];
        let melody_wave = if melody_frequency > 0.0 {
            let envelope = (-eighth_phase * 8.5).exp();
            let fundamental = (2.0 * PI * melody_frequency * time).sin();
            let octave = (2.0 * PI * melody_frequency * 2.0 * time).sin() * 0.22;
            (fundamental + octave) * envelope * 0.11
        } else {
            0.0
        };

        let impact = impact_cues.iter().fold(0.0_f32, |sum, cue| {
            let phase = time - cue;
            if (0.0..0.48).contains(&phase) {
                let drop_frequency = 180.0 - 120.0 * (phase / 0.48);
                let tone = (2.0 * PI * drop_frequency * phase).sin();
                sum + (tone * 0.22 + noise * 0.11) * (-phase * 7.0).exp()
            } else {
                sum
            }
        });

        let master = (kick + snare + hat + bass_wave + melody_wave + impact).tanh() * 0.78;
        let left = (master * 32_000.0).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        let right_sample =
            (kick + snare * 0.9 + hat * 1.08 + bass_wave + melody_wave * 0.94 + impact).tanh()
                * 0.76;
        let right = (right_sample * 32_000.0).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        writer.write_all(&left.to_le_bytes())?;
        writer.write_all(&right.to_le_bytes())?;
    }

    writer.flush()
}

#[cfg(test)]
mod tests {
    use super::write_original_bgm;

    #[test]
    fn original_bgm_is_a_stereo_wave_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("bgm.wav");
        write_original_bgm(&path, 0.1, &[0.05]).unwrap();
        let bytes = std::fs::read(path).unwrap();

        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert!(bytes.len() > 19_000);
    }
}
