// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

// spec: flp/container.md (Alignment checks)

use crate::container::FlpFile;
use crate::event::Value;

/// Shortest run of text-like ID-0 events that counts as misalignment.
pub const TEXT_RUN_MIN: usize = 6;

/// The event that marks a channel; its count must equal the header channel count.
const CHANNEL_EVENT: u8 = 64;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AlignmentIssue {
    #[error("no FLhd chunk with a channel count, so the event-64 count cannot be checked")]
    NoChannelCount,

    #[error("header says {header} channels but the stream has {found} event-64s")]
    ChannelCountMismatch { header: u16, found: usize },

    #[error(
        "{len} consecutive ID-0 events with printable ASCII values starting at offset {offset:#x} (text read as events?)"
    )]
    TextLikeRun { offset: usize, len: usize },
}

/// Runs both alignment checks. An empty result means the stream looks well aligned.
pub fn check_alignment(file: &FlpFile) -> Vec<AlignmentIssue> {
    let mut issues = Vec::new();

    let found = file.events().filter(|e| e.id == CHANNEL_EVENT).count();
    match file.header().and_then(|h| h.channel_count()) {
        None => issues.push(AlignmentIssue::NoChannelCount),
        Some(header) if usize::from(header) != found => {
            issues.push(AlignmentIssue::ChannelCountMismatch { header, found })
        }
        Some(_) => {}
    }

    for (chunk_at, chunk) in file.chunk_offsets() {
        let mut run_start = 0;
        let mut run_len = 0;
        for (at, e) in chunk.events_at(chunk_at) {
            let text_like =
                e.id == 0 && matches!(e.value, Value::U8(v) if (0x20..=0x7e).contains(&v));
            if text_like {
                if run_len == 0 {
                    run_start = at;
                }
                run_len += 1;
            } else {
                flush_run(&mut issues, run_start, &mut run_len);
            }
        }
        flush_run(&mut issues, run_start, &mut run_len);
    }

    issues
}

fn flush_run(issues: &mut Vec<AlignmentIssue>, offset: usize, len: &mut usize) {
    if *len >= TEXT_RUN_MIN {
        issues.push(AlignmentIssue::TextLikeRun { offset, len: *len });
    }
    *len = 0;
}
