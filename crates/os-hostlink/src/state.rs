// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! A Lemon plugin's state blob, as the host and the plugins read it: nih-plug's JSON, with or without an 8-byte
//! little-endian length before it.

/// The JSON of state `blob`, and whether a length came before it. The length is told by its value, not by the first
/// byte: a length's low byte can itself be `{` (a 0x7B, 0x17B... byte JSON).
pub fn json(blob: &[u8]) -> (bool, &[u8]) {
    match blob.split_first_chunk::<8>() {
        Some((len, json)) if u64::from_le_bytes(*len) == json.len() as u64 => (true, json),
        _ => (false, blob),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_length_is_told_by_its_value() {
        let text = format!(r#"{{"fields":{{"x":"{}"}}}}"#, "a".repeat(0x17B - 19));
        assert_eq!(text.len(), 0x17B);
        let mut blob = (text.len() as u64).to_le_bytes().to_vec();
        blob.extend_from_slice(text.as_bytes());
        assert_eq!(blob[0], b'{');
        assert_eq!(json(&blob), (true, text.as_bytes()));
        assert_eq!(json(text.as_bytes()), (false, text.as_bytes()));
        assert_eq!(json(b"{}"), (false, &b"{}"[..]));
        assert_eq!(json(&[]), (false, &[][..]));
    }
}
