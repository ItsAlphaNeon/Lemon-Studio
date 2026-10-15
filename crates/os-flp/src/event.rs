// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

// spec: flp/container.md (Events; Exceptions to the range rule; Variable-length prefix)
// Kind labels agree with flp/coverage.md.

use std::fmt;

use crate::error::ReadError;

/// How an event's value is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    U8,
    U16,
    U32,
    /// LEB128 length prefix, then that many payload bytes.
    Var,
}

impl Kind {
    /// Value size in bytes for fixed-size kinds.
    pub fn fixed_size(self) -> Option<usize> {
        match self {
            Kind::U8 => Some(1),
            Kind::U16 => Some(2),
            Kind::U32 => Some(4),
            Kind::Var => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::U8 => "u8",
            Kind::U16 => "u16",
            Kind::U32 => "u32",
            Kind::Var => "var",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// IDs whose value size differs from the range rule. Applied before [`range_kind`].
pub const SIZE_EXCEPTIONS: &[(u8, Kind)] = &[(172, Kind::U8)];

/// The kind implied by the ID range alone, ignoring [`SIZE_EXCEPTIONS`].
pub const fn range_kind(id: u8) -> Kind {
    match id {
        0..=63 => Kind::U8,
        64..=127 => Kind::U16,
        128..=191 => Kind::U32,
        192..=255 => Kind::Var,
    }
}

/// The kind of an event ID: the exceptions table first, then the range rule.
pub fn kind_of(id: u8) -> Kind {
    SIZE_EXCEPTIONS
        .iter()
        .find(|&&(x, _)| x == id)
        .map_or_else(|| range_kind(id), |&(_, k)| k)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    U8(u8),
    U16(u16),
    U32(u32),
    Var {
        data: Vec<u8>,
        /// The exact length-prefix bytes read from the file. On write they are reused only
        /// if they still decode to `data.len()`; otherwise a minimal prefix is written.
        /// `None` for newly created events.
        prefix: Option<Vec<u8>>,
    },
}

impl Value {
    pub fn kind(&self) -> Kind {
        match self {
            Value::U8(_) => Kind::U8,
            Value::U16(_) => Kind::U16,
            Value::U32(_) => Kind::U32,
            Value::Var { .. } => Kind::Var,
        }
    }

    /// A new variable-length value (minimal prefix on write).
    pub fn var(data: impl Into<Vec<u8>>) -> Self {
        Value::Var {
            data: data.into(),
            prefix: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: u8,
    pub value: Value,
}

impl Event {
    pub fn new(id: u8, value: Value) -> Self {
        Event { id, value }
    }

    pub fn kind(&self) -> Kind {
        self.value.kind()
    }

    /// Fixed values widened to u32; `None` for variable-length events.
    pub fn int_value(&self) -> Option<u32> {
        match self.value {
            Value::U8(v) => Some(v.into()),
            Value::U16(v) => Some(v.into()),
            Value::U32(v) => Some(v),
            Value::Var { .. } => None,
        }
    }

    /// Payload of a variable-length event.
    pub fn data(&self) -> Option<&[u8]> {
        match &self.value {
            Value::Var { data, .. } => Some(data),
            _ => None,
        }
    }

    /// The length-prefix bytes this event will be written with.
    pub fn prefix_bytes(&self) -> Option<Vec<u8>> {
        match &self.value {
            Value::Var { data, prefix } => Some(match prefix {
                Some(p) if prefix_is_valid_for(p, data.len()) => p.clone(),
                _ => encode_varint(data.len() as u64),
            }),
            _ => None,
        }
    }

    /// Serialized size: ID byte + value (+ prefix).
    pub fn encoded_len(&self) -> usize {
        1 + match &self.value {
            Value::U8(_) => 1,
            Value::U16(_) => 2,
            Value::U32(_) => 4,
            Value::Var { data, .. } => self.prefix_bytes().map_or(0, |p| p.len()) + data.len(),
        }
    }

    /// Appends the serialized event. The caller checks the kind against the ID.
    pub(crate) fn encode_into(&self, out: &mut Vec<u8>) {
        out.push(self.id);
        match &self.value {
            Value::U8(v) => out.push(*v),
            Value::U16(v) => out.extend_from_slice(&v.to_le_bytes()),
            Value::U32(v) => out.extend_from_slice(&v.to_le_bytes()),
            Value::Var { data, .. } => {
                out.extend_from_slice(&self.prefix_bytes().unwrap_or_default());
                out.extend_from_slice(data);
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum VarintError {
    Truncated,
    Overflow,
}

/// Decodes an unsigned LEB128 value; returns (value, bytes consumed).
/// Non-minimal encodings are accepted.
pub(crate) fn read_varint(buf: &[u8]) -> Result<(u64, usize), VarintError> {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    for (i, &b) in buf.iter().enumerate() {
        let group = u64::from(b & 0x7f);
        if shift >= 64 {
            if group != 0 {
                return Err(VarintError::Overflow);
            }
        } else {
            if (group << shift) >> shift != group {
                return Err(VarintError::Overflow);
            }
            value |= group << shift;
        }
        if b & 0x80 == 0 {
            return Ok((value, i + 1));
        }
        shift = shift.saturating_add(7);
    }
    Err(VarintError::Truncated)
}

/// Minimal unsigned LEB128 encoding.
pub(crate) fn encode_varint(mut v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return out;
        }
        out.push(b | 0x80);
    }
}

fn prefix_is_valid_for(prefix: &[u8], len: usize) -> bool {
    matches!(read_varint(prefix), Ok((v, n)) if n == prefix.len() && v == len as u64)
}

/// Walks the events of an `FLdt` payload, yielding each event with its absolute offset.
/// Stops after the first error.
pub struct EventReader<'a> {
    buf: &'a [u8],
    pos: usize,
    base: usize,
    done: bool,
}

impl<'a> EventReader<'a> {
    /// `base` is the absolute file offset of `payload[0]`, used in offsets and errors.
    pub fn new(payload: &'a [u8], base: usize) -> Self {
        EventReader {
            buf: payload,
            pos: 0,
            base,
            done: false,
        }
    }

    fn read_one(&mut self) -> Result<(usize, Event), ReadError> {
        let start = self.pos;
        let offset = self.base + start;
        let id = self.buf[start];
        let rest = &self.buf[start + 1..];
        let kind = kind_of(id);
        let (value, used) = match kind.fixed_size() {
            Some(n) => {
                let Some(b) = rest.get(..n) else {
                    return Err(ReadError::EventOverrun {
                        offset,
                        id,
                        needed: n as u64,
                        available: rest.len(),
                    });
                };
                let v = match kind {
                    Kind::U8 => Value::U8(b[0]),
                    Kind::U16 => Value::U16(u16::from_le_bytes([b[0], b[1]])),
                    _ => Value::U32(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                };
                (v, n)
            }
            None => {
                let (len, plen) = read_varint(rest).map_err(|e| match e {
                    VarintError::Truncated => ReadError::PrefixTruncated { offset, id },
                    VarintError::Overflow => ReadError::PrefixOverflow { offset, id },
                })?;
                let after = &rest[plen..];
                let data = usize::try_from(len)
                    .ok()
                    .and_then(|n| after.get(..n))
                    .ok_or(ReadError::EventOverrun {
                        offset,
                        id,
                        needed: len,
                        available: after.len(),
                    })?;
                let v = Value::Var {
                    data: data.to_vec(),
                    prefix: Some(rest[..plen].to_vec()),
                };
                (v, plen + data.len())
            }
        };
        self.pos = start + 1 + used;
        Ok((offset, Event { id, value }))
    }
}

impl Iterator for EventReader<'_> {
    type Item = Result<(usize, Event), ReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.pos >= self.buf.len() {
            return None;
        }
        let r = self.read_one();
        self.done = r.is_err();
        Some(r)
    }
}
