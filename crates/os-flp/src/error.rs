// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

// spec: flp/container.md (Conformance requirements, item 3)

use crate::container::Tag;
use crate::event::Kind;

/// A malformed file. Every variant carries the absolute byte offset of the offending
/// chunk or event (the position of its tag / ID byte).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReadError {
    #[error(
        "chunk {tag} at offset {offset:#x}: declared length {declared} overruns the file ({available} bytes left)"
    )]
    ChunkOverrun {
        offset: usize,
        tag: Tag,
        declared: u32,
        available: usize,
    },

    #[error(
        "event {id} at offset {offset:#x}: value needs {needed} bytes, only {available} left in the chunk"
    )]
    EventOverrun {
        offset: usize,
        id: u8,
        needed: u64,
        available: usize,
    },

    #[error("event {id} at offset {offset:#x}: length prefix runs past the end of the chunk")]
    PrefixTruncated { offset: usize, id: u8 },

    #[error("event {id} at offset {offset:#x}: length prefix does not fit in 64 bits")]
    PrefixOverflow { offset: usize, id: u8 },
}

impl ReadError {
    /// Absolute byte offset of the malformed chunk or event.
    pub fn offset(&self) -> usize {
        match *self {
            ReadError::ChunkOverrun { offset, .. }
            | ReadError::EventOverrun { offset, .. }
            | ReadError::PrefixTruncated { offset, .. }
            | ReadError::PrefixOverflow { offset, .. } => offset,
        }
    }
}

/// A model that cannot be serialized.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WriteError {
    #[error(
        "event #{index} (id {id}) in chunk {tag} holds a {got} value, but id {id} is {expected}"
    )]
    KindMismatch {
        tag: Tag,
        index: usize,
        id: u8,
        expected: Kind,
        got: Kind,
    },

    #[error("chunk {tag} payload is {len} bytes, more than a 32-bit length can describe")]
    ChunkTooLarge { tag: Tag, len: usize },
}
