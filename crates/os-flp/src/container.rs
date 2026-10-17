// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

// spec: flp/container.md (Chunks; FLhd payload; FLdt payload; Conformance requirements)

use std::fmt;

use crate::error::{ReadError, WriteError};
use crate::event::{Event, EventReader, kind_of};

/// A 4-byte chunk tag.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tag(pub [u8; 4]);

impl Tag {
    pub const HEADER: Tag = Tag(*b"FLhd");
    pub const DATA: Tag = Tag(*b"FLdt");
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for &b in &self.0 {
            if b.is_ascii_graphic() || b == b' ' {
                write!(f, "{}", b as char)?;
            } else {
                write!(f, "\\x{b:02x}")?;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Tag({self})")
    }
}

/// `FLhd` payload. The raw bytes are kept as-is, whatever their length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub raw: Vec<u8>,
}

impl Header {
    fn u16_at(&self, at: usize) -> Option<u16> {
        self.raw
            .get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn format(&self) -> Option<i16> {
        self.u16_at(0).map(|v| v as i16)
    }

    pub fn channel_count(&self) -> Option<u16> {
        self.u16_at(2)
    }

    /// Ticks per quarter note.
    pub fn ppq(&self) -> Option<u16> {
        self.u16_at(4)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkBody {
    /// An `FLhd` chunk.
    Header(Header),
    /// An `FLdt` chunk: the event stream, in file order.
    Events(Vec<Event>),
    /// Any other chunk, preserved verbatim.
    Raw(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub tag: Tag,
    pub body: ChunkBody,
}

/// A whole FLP file: chunks in file order plus any trailing bytes too short to be a chunk.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlpFile {
    pub chunks: Vec<Chunk>,
    pub trailing: Vec<u8>,
}

const CHUNK_HEADER_LEN: usize = 8;

impl FlpFile {
    /// Tokenizes a file. Never panics; malformed input yields a [`ReadError`] with its offset.
    pub fn parse(bytes: &[u8]) -> Result<Self, ReadError> {
        let mut chunks = Vec::new();
        let mut pos = 0;
        while bytes.len() - pos >= CHUNK_HEADER_LEN {
            let tag = Tag(bytes[pos..pos + 4].try_into().unwrap());
            let declared = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap());
            let start = pos + CHUNK_HEADER_LEN;
            let available = bytes.len() - start;
            let len = declared as usize;
            if len > available {
                return Err(ReadError::ChunkOverrun {
                    offset: pos,
                    tag,
                    declared,
                    available,
                });
            }
            let payload = &bytes[start..start + len];
            let body = match tag {
                Tag::HEADER => ChunkBody::Header(Header {
                    raw: payload.to_vec(),
                }),
                Tag::DATA => ChunkBody::Events(
                    EventReader::new(payload, start)
                        .map(|r| r.map(|(_, e)| e))
                        .collect::<Result<_, _>>()?,
                ),
                _ => ChunkBody::Raw(payload.to_vec()),
            };
            chunks.push(Chunk { tag, body });
            pos = start + len;
        }
        Ok(FlpFile {
            chunks,
            trailing: bytes[pos..].to_vec(),
        })
    }

    /// Serializes the file. For a parsed, unmodified file this reproduces the input exactly.
    pub fn to_bytes(&self) -> Result<Vec<u8>, WriteError> {
        let mut out = Vec::new();
        for chunk in &self.chunks {
            let tag = chunk.tag;
            out.extend_from_slice(&tag.0);
            let len_at = out.len();
            out.extend_from_slice(&[0; 4]);
            match &chunk.body {
                ChunkBody::Header(h) => out.extend_from_slice(&h.raw),
                ChunkBody::Raw(raw) => out.extend_from_slice(raw),
                ChunkBody::Events(events) => {
                    for (index, e) in events.iter().enumerate() {
                        let expected = kind_of(e.id);
                        if e.kind() != expected {
                            return Err(WriteError::KindMismatch {
                                tag,
                                index,
                                id: e.id,
                                expected,
                                got: e.kind(),
                            });
                        }
                        e.encode_into(&mut out);
                    }
                }
            }
            let len = out.len() - len_at - 4;
            let len32 = u32::try_from(len).map_err(|_| WriteError::ChunkTooLarge { tag, len })?;
            out[len_at..len_at + 4].copy_from_slice(&len32.to_le_bytes());
        }
        out.extend_from_slice(&self.trailing);
        Ok(out)
    }

    /// The first `FLhd` chunk's payload.
    pub fn header(&self) -> Option<&Header> {
        self.chunks.iter().find_map(|c| match &c.body {
            ChunkBody::Header(h) => Some(h),
            _ => None,
        })
    }

    /// All events of all `FLdt` chunks, in file order.
    pub fn events(&self) -> impl Iterator<Item = &Event> {
        self.chunks.iter().flat_map(|c| match &c.body {
            ChunkBody::Events(ev) => ev.as_slice(),
            _ => &[],
        })
    }

    /// Each chunk with the offset of its tag in the serialized file. For a parsed,
    /// unmodified file these are the source offsets.
    pub fn chunk_offsets(&self) -> Vec<(usize, &Chunk)> {
        let mut pos = 0;
        self.chunks
            .iter()
            .map(|c| {
                let at = pos;
                pos += CHUNK_HEADER_LEN + c.payload_len();
                (at, c)
            })
            .collect()
    }
}

impl Chunk {
    /// Serialized payload length (excluding the 8-byte tag/length header).
    pub fn payload_len(&self) -> usize {
        match &self.body {
            ChunkBody::Header(h) => h.raw.len(),
            ChunkBody::Raw(raw) => raw.len(),
            ChunkBody::Events(ev) => ev.iter().map(Event::encoded_len).sum(),
        }
    }

    /// Events with their serialized offsets, given this chunk's tag offset.
    pub fn events_at(&self, chunk_offset: usize) -> Vec<(usize, &Event)> {
        let ChunkBody::Events(events) = &self.body else {
            return Vec::new();
        };
        let mut pos = chunk_offset + CHUNK_HEADER_LEN;
        events
            .iter()
            .map(|e| {
                let at = pos;
                pos += e.encoded_len();
                (at, e)
            })
            .collect()
    }
}
