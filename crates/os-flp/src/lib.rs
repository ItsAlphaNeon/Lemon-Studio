// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! FLP project file support.
//!
//! Implemented from the specs' `flp/` pages only (clean-room rules).
//! Requirement: read → write must reproduce the input byte-for-byte; unknown events are
//! preserved verbatim.
//!
//! - [`FlpFile::parse`] / [`FlpFile::to_bytes`]: the lossless container layer.
//! - [`check_alignment`]: the stream-alignment checks every test over real files should run,
//!   because a wrong event size can still round-trip byte-identically.

// spec: flp/container.md

mod check;
mod container;
mod error;
mod event;
pub mod project;
pub mod wrapper;

pub use check::{AlignmentIssue, TEXT_RUN_MIN, check_alignment};
pub use container::{Chunk, ChunkBody, FlpFile, Header, Tag};
pub use error::{ReadError, WriteError};
pub use event::{Event, EventReader, Kind, SIZE_EXCEPTIONS, Value, kind_of, range_kind};

#[cfg(test)]
mod tests;
