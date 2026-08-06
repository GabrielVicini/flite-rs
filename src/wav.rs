//! Minimal RIFF/WAVE writer for 16-bit mono PCM.
//!
//! Writing the header directly costs about forty lines and removes a
//! dependency from a crate whose whole point is portability.

use std::io::{self, Seek, SeekFrom, Write};

use crate::voice::Audio;

const BITS_PER_SAMPLE: u16 = 16;
const CHANNELS: u16 = 1;
const PCM_FORMAT: u16 = 1;

/// The 44 bytes before the samples. `data_bytes` has to be known in advance,
/// which is the whole difficulty with writing this format as you go.
fn write_header<W: Write>(out: &mut W, sample_rate: u32, data_bytes: u32) -> io::Result<()> {
    let byte_rate = sample_rate * u32::from(CHANNELS) * u32::from(BITS_PER_SAMPLE) / 8;
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
    out.write_all(&sample_rate.to_le_bytes())?;
    out.write_all(&byte_rate.to_le_bytes())?;
    out.write_all(&block_align.to_le_bytes())?;
    out.write_all(&BITS_PER_SAMPLE.to_le_bytes())?;

    out.write_all(b"data")?;
    out.write_all(&data_bytes.to_le_bytes())
}

/// Serialise audio as a RIFF/WAVE file.
///
/// Writes little-endian regardless of host byte order, so the output is
/// identical on every platform.
pub fn write_wav<W: Write>(audio: &Audio, out: &mut W) -> io::Result<()> {
    write_header(out, audio.sample_rate, audio.samples.len() as u32 * 2)?;

    // Buffer the samples so a slow writer does not see thousands of tiny
    // writes.
    let mut buffer = Vec::with_capacity(audio.samples.len() * 2);
    for sample in &audio.samples {
        buffer.extend_from_slice(&sample.to_le_bytes());
    }
    out.write_all(&buffer)
}

/// A WAV file written as the samples arrive.
///
/// The two length fields cannot be known until the end, so they start at zero
/// and are rewritten by [`WavWriter::finish`]. That needs a seekable
/// destination, which a file is and a pipe is not; for a pipe, collect the
/// audio and use [`write_wav`].
pub struct WavWriter<W: Write + Seek> {
    out: W,
    sample_rate: u32,
    samples: u32,
}

impl<W: Write + Seek> WavWriter<W> {
    pub fn new(mut out: W, sample_rate: u32) -> io::Result<WavWriter<W>> {
        write_header(&mut out, sample_rate, 0)?;
        Ok(WavWriter {
            out,
            sample_rate,
            samples: 0,
        })
    }

    pub fn write(&mut self, samples: &[i16]) -> io::Result<()> {
        let mut buffer = Vec::with_capacity(samples.len() * 2);
        for sample in samples {
            buffer.extend_from_slice(&sample.to_le_bytes());
        }
        self.samples += samples.len() as u32;
        self.out.write_all(&buffer)
    }

    /// Rewrite the header now that the length is known.
    ///
    /// Skipping this leaves a file that says it holds no audio, so it is not
    /// optional.
    pub fn finish(mut self) -> io::Result<W> {
        self.out.seek(SeekFrom::Start(0))?;
        write_header(&mut self.out, self.sample_rate, self.samples * 2)?;
        self.out.seek(SeekFrom::End(0))?;
        self.out.flush()?;
        Ok(self.out)
    }
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

    #[test]
    fn streamed_output_is_byte_for_byte_the_buffered_output() {
        let audio = Audio {
            samples: vec![0, 1, -1, 32767, -32768, 7],
            sample_rate: 8000,
        };
        let mut buffered = Vec::new();
        write_wav(&audio, &mut buffered).unwrap();

        let mut writer = WavWriter::new(io::Cursor::new(Vec::new()), audio.sample_rate).unwrap();
        // In pieces, since that is the point of it.
        for chunk in audio.samples.chunks(2) {
            writer.write(chunk).unwrap();
        }
        assert_eq!(writer.finish().unwrap().into_inner(), buffered);
    }
}
