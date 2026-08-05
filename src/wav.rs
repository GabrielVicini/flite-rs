//! Minimal RIFF/WAVE writer for 16-bit mono PCM.
//!
//! Writing the header directly costs about forty lines and removes a
//! dependency from a crate whose whole point is portability.

use std::io::{self, Write};

use crate::voice::Audio;

/// Serialise audio as a RIFF/WAVE file.
///
/// Writes little-endian regardless of host byte order, so the output is
/// identical on every platform.
pub fn write_wav<W: Write>(audio: &Audio, out: &mut W) -> io::Result<()> {
    const BITS_PER_SAMPLE: u16 = 16;
    const CHANNELS: u16 = 1;
    const PCM_FORMAT: u16 = 1;

    let data_bytes = audio.samples.len() as u32 * 2;
    let byte_rate = audio.sample_rate * u32::from(CHANNELS) * u32::from(BITS_PER_SAMPLE) / 8;
    let block_align = CHANNELS * BITS_PER_SAMPLE / 8;

    out.write_all(b"RIFF")?;
    // Everything after this field: the 4-byte "WAVE" tag, the 24-byte fmt
    // chunk, the 8-byte data header, and the samples.
    out.write_all(&(36 + data_bytes).to_le_bytes())?;
    out.write_all(b"WAVE")?;

    out.write_all(b"fmt ")?;
    out.write_all(&16u32.to_le_bytes())?;
    out.write_all(&PCM_FORMAT.to_le_bytes())?;
    out.write_all(&CHANNELS.to_le_bytes())?;
    out.write_all(&audio.sample_rate.to_le_bytes())?;
    out.write_all(&byte_rate.to_le_bytes())?;
    out.write_all(&block_align.to_le_bytes())?;
    out.write_all(&BITS_PER_SAMPLE.to_le_bytes())?;

    out.write_all(b"data")?;
    out.write_all(&data_bytes.to_le_bytes())?;

    // Buffer the samples so a slow writer does not see thousands of tiny
    // writes.
    let mut buffer = Vec::with_capacity(audio.samples.len() * 2);
    for sample in &audio.samples {
        buffer.extend_from_slice(&sample.to_le_bytes());
    }
    out.write_all(&buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_describes_the_payload() {
        let audio = Audio {
            samples: vec![0, 1, -1, 32767],
            sample_rate: 8000,
        };
        let mut bytes = Vec::new();
        write_wav(&audio, &mut bytes).unwrap();

        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 8);
        assert_eq!(bytes.len(), 44 + 8);
    }
}
