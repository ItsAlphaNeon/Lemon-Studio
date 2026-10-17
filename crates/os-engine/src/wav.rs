// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Minimal WAV writer (32-bit float, stereo).

use std::io::{self, Write};

/// Writes interleaved stereo `f32` samples as a 32-bit IEEE float WAV file.
pub fn write_f32_stereo(w: &mut impl Write, rate: u32, interleaved: &[f32]) -> io::Result<()> {
    const CHANNELS: u16 = 2;
    const BITS: u16 = 32;
    let data_len = u32::try_from(interleaved.len() * 4)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "render too long for WAV"))?;
    let block_align = CHANNELS * BITS / 8;
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data_len).to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&3u16.to_le_bytes())?; // WAVE_FORMAT_IEEE_FLOAT
    w.write_all(&CHANNELS.to_le_bytes())?;
    w.write_all(&rate.to_le_bytes())?;
    w.write_all(&(rate * u32::from(block_align)).to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&BITS.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_len.to_le_bytes())?;
    for s in interleaved {
        w.write_all(&s.to_le_bytes())?;
    }
    Ok(())
}
