// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

// spec: flp/container.md — unit tests over hand-built byte streams.

use crate::event::{VarintError, encode_varint, read_varint};
use crate::*;

fn chunk(tag: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = tag.to_vec();
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn header(format: i16, channels: u16, ppq: u16) -> Vec<u8> {
    let mut p = format.to_le_bytes().to_vec();
    p.extend_from_slice(&channels.to_le_bytes());
    p.extend_from_slice(&ppq.to_le_bytes());
    chunk(b"FLhd", &p)
}

fn flp(channels: u16, events: &[u8]) -> Vec<u8> {
    let mut out = header(0, channels, 96);
    out.extend(chunk(b"FLdt", events));
    out
}

/// Parses, checks byte-identical re-serialization, returns the parsed file.
fn roundtrip(bytes: &[u8]) -> FlpFile {
    let f = FlpFile::parse(bytes).expect("parse");
    assert_eq!(f.to_bytes().expect("write"), bytes, "round-trip differs");
    f
}

fn events(f: &FlpFile) -> Vec<Event> {
    f.events().cloned().collect()
}

/// Offset of the FLdt payload in files built by `flp`: 14-byte FLhd chunk + 8.
const EV0: usize = 22;

// ---- kinds -----------------------------------------------------------------

#[test]
fn range_rule() {
    assert_eq!(range_kind(0), Kind::U8);
    assert_eq!(range_kind(63), Kind::U8);
    assert_eq!(range_kind(64), Kind::U16);
    assert_eq!(range_kind(127), Kind::U16);
    assert_eq!(range_kind(128), Kind::U32);
    assert_eq!(range_kind(191), Kind::U32);
    assert_eq!(range_kind(192), Kind::Var);
    assert_eq!(range_kind(255), Kind::Var);
}

#[test]
fn exceptions_table_applies_before_range_rule() {
    assert_eq!(kind_of(172), Kind::U8);
    for id in 0..=255u8 {
        if id != 172 {
            assert_eq!(kind_of(id), range_kind(id), "id {id}");
        }
    }
}

#[test]
fn kinds_match_coverage_labels() {
    // Spot checks of the `kind` column in flp/coverage.md, including the boundary IDs seen.
    let labels = [
        (0, "u8"),
        (52, "u8"),
        (64, "u16"),
        (104, "u16"),
        (128, "u32"),
        (171, "u32"),
        (172, "u8"),
        (192, "var"),
        (213, "var"),
        (253, "var"),
    ];
    for (id, label) in labels {
        assert_eq!(kind_of(id).label(), label, "id {id}");
    }
}

// ---- events ----------------------------------------------------------------

#[test]
fn fixed_values_are_little_endian() {
    let bytes = flp(0, &[5, 0xAB, 70, 0x34, 0x12, 156, 0x78, 0x56, 0x34, 0x12]);
    let f = roundtrip(&bytes);
    assert_eq!(
        events(&f),
        vec![
            Event::new(5, Value::U8(0xAB)),
            Event::new(70, Value::U16(0x1234)),
            Event::new(156, Value::U32(0x1234_5678)),
        ]
    );
}

#[test]
fn event_172_has_a_one_byte_value() {
    // 172 then a var event 192 "hi". Under the range rule 172 would swallow 192 and the
    // prefix, misaligning everything after it.
    let bytes = flp(0, &[172, 1, 192, 2, b'h', b'i']);
    let f = roundtrip(&bytes);
    let ev = events(&f);
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[0], Event::new(172, Value::U8(1)));
    assert_eq!(ev[1].id, 192);
    assert_eq!(ev[1].data(), Some(&b"hi"[..]));
}

#[test]
fn var_event_with_multibyte_prefix() {
    let payload = vec![0x5A; 300];
    let mut stream = vec![213];
    stream.extend(encode_varint(300));
    assert_eq!(&stream[1..], &[0xAC, 0x02]);
    stream.extend(&payload);
    let f = roundtrip(&flp(0, &stream));
    assert_eq!(events(&f)[0].data(), Some(&payload[..]));
}

#[test]
fn zero_length_var_events() {
    let f = roundtrip(&flp(0, &[216, 0, 233, 0]));
    let ev = events(&f);
    assert_eq!(ev.len(), 2);
    assert!(ev.iter().all(|e| e.data() == Some(&[][..])));
}

#[test]
fn non_minimal_prefix_is_preserved() {
    // Length 2 encoded in three bytes.
    let bytes = flp(0, &[200, 0x82, 0x80, 0x00, b'o', b'k']);
    let f = roundtrip(&bytes);
    let e = &events(&f)[0];
    assert_eq!(e.data(), Some(&b"ok"[..]));
    assert_eq!(e.prefix_bytes(), Some(vec![0x82, 0x80, 0x00]));
}

#[test]
fn stale_prefix_is_replaced_by_minimal_encoding() {
    let mut f = FlpFile::parse(&flp(0, &[200, 0x82, 0x80, 0x00, b'o', b'k'])).unwrap();
    let ChunkBody::Events(ev) = &mut f.chunks[1].body else {
        panic!()
    };
    let Value::Var { data, .. } = &mut ev[0].value else {
        panic!()
    };
    data.push(b'!');
    assert_eq!(f.to_bytes().unwrap(), flp(0, &[200, 3, b'o', b'k', b'!']));
}

#[test]
fn new_events_use_minimal_prefix() {
    let f = FlpFile {
        chunks: vec![Chunk {
            tag: Tag::DATA,
            body: ChunkBody::Events(vec![Event::new(201, Value::var(vec![7; 128]))]),
        }],
        trailing: vec![],
    };
    let out = f.to_bytes().unwrap();
    assert_eq!(&out[8..11], &[201, 0x80, 0x01]);
    assert_eq!(out.len(), 8 + 3 + 128);
}

#[test]
fn unknown_ids_are_preserved() {
    // IDs absent from coverage.md in each range.
    roundtrip(&flp(0, &[2, 9, 66, 1, 2, 130, 1, 2, 3, 4, 255, 1, 0xEE]));
}

#[test]
fn writer_rejects_value_of_wrong_kind() {
    let f = FlpFile {
        chunks: vec![Chunk {
            tag: Tag::DATA,
            body: ChunkBody::Events(vec![Event::new(172, Value::U32(1))]),
        }],
        trailing: vec![],
    };
    assert_eq!(
        f.to_bytes(),
        Err(WriteError::KindMismatch {
            tag: Tag::DATA,
            index: 0,
            id: 172,
            expected: Kind::U8,
            got: Kind::U32
        })
    );
}

#[test]
fn varint_decoding() {
    assert_eq!(read_varint(&[0x00]), Ok((0, 1)));
    assert_eq!(read_varint(&[0x7F]), Ok((127, 1)));
    assert_eq!(read_varint(&[0x80, 0x01]), Ok((128, 2)));
    assert_eq!(read_varint(&[0xA0, 0xE6, 0x81, 0x03]), Ok((6_320_928, 4)));
    assert_eq!(read_varint(&[0x80]), Err(VarintError::Truncated));
    assert_eq!(read_varint(&[]), Err(VarintError::Truncated));
    let max = encode_varint(u64::MAX);
    assert_eq!(read_varint(&max), Ok((u64::MAX, 10)));
    let mut over = max.clone();
    *over.last_mut().unwrap() = 0x02; // bit 64
    assert_eq!(read_varint(&over), Err(VarintError::Overflow));
    // Non-minimal padding past 64 bits is fine as long as it carries no bits.
    let mut padded = vec![0x81];
    padded.extend([0x80; 12]);
    padded.push(0x00);
    assert_eq!(read_varint(&padded), Ok((1, 14)));
    for v in [0, 1, 127, 128, 16383, 16384, 6_320_928, u64::from(u32::MAX)] {
        let enc = encode_varint(v);
        assert_eq!(read_varint(&enc), Ok((v, enc.len())));
    }
}

// ---- chunks ----------------------------------------------------------------

#[test]
fn header_fields() {
    let f = roundtrip(&header(0, 12, 96));
    let h = f.header().unwrap();
    assert_eq!(
        (h.format(), h.channel_count(), h.ppq()),
        (Some(0), Some(12), Some(96))
    );
}

#[test]
fn long_and_short_headers_are_preserved() {
    let long = chunk(b"FLhd", &[0, 0, 3, 0, 48, 0, 0xDE, 0xAD]);
    let f = roundtrip(&long);
    assert_eq!(f.header().unwrap().ppq(), Some(48));

    let short = chunk(b"FLhd", &[0, 0, 3]);
    let f = roundtrip(&short);
    let h = f.header().unwrap();
    assert_eq!(
        (h.format(), h.channel_count(), h.ppq()),
        (Some(0), None, None)
    );
}

#[test]
fn unknown_chunks_and_trailing_bytes_are_preserved() {
    let mut bytes = header(0, 0, 96);
    bytes.extend(chunk(b"XTRA", &[1, 2, 3]));
    bytes.extend(chunk(b"FLdt", &[9, 1]));
    bytes.extend(chunk(b"ZZZZ", &[]));
    bytes.extend([0xFF, 0xFE, 0xFD]);
    let f = roundtrip(&bytes);
    let tags: Vec<_> = f.chunks.iter().map(|c| c.tag).collect();
    assert_eq!(tags, [Tag::HEADER, Tag(*b"XTRA"), Tag::DATA, Tag(*b"ZZZZ")]);
    assert_eq!(f.trailing, [0xFF, 0xFE, 0xFD]);
}

#[test]
fn empty_file() {
    let f = roundtrip(&[]);
    assert!(f.chunks.is_empty());
}

#[test]
fn offsets_match_source_layout() {
    let bytes = flp(0, &[199, 2, b'1', 0, 64, 0, 0, 172, 0]);
    let f = roundtrip(&bytes);
    let co = f.chunk_offsets();
    assert_eq!(co.iter().map(|(o, _)| *o).collect::<Vec<_>>(), [0, 14]);
    let at: Vec<_> = co[1]
        .1
        .events_at(co[1].0)
        .iter()
        .map(|(o, e)| (*o, e.id))
        .collect();
    assert_eq!(at, [(EV0, 199), (EV0 + 4, 64), (EV0 + 7, 172)]);
}

// ---- errors ----------------------------------------------------------------

#[test]
fn chunk_overrun_reports_offset() {
    let mut bytes = header(0, 0, 96);
    bytes.extend(b"FLdt");
    bytes.extend(10u32.to_le_bytes());
    bytes.extend([0; 4]);
    let err = FlpFile::parse(&bytes).unwrap_err();
    assert_eq!(
        err,
        ReadError::ChunkOverrun {
            offset: 14,
            tag: Tag::DATA,
            declared: 10,
            available: 4
        }
    );
    assert_eq!(err.offset(), 14);
    assert!(err.to_string().contains("0xe"), "{err}");
}

#[test]
fn fixed_event_overrun_reports_offset() {
    let err = FlpFile::parse(&flp(0, &[9, 1, 156, 1, 2])).unwrap_err();
    assert_eq!(
        err,
        ReadError::EventOverrun {
            offset: EV0 + 2,
            id: 156,
            needed: 4,
            available: 2
        }
    );
}

#[test]
fn id_172_at_end_needs_only_one_byte() {
    roundtrip(&flp(0, &[172, 0]));
    let err = FlpFile::parse(&flp(0, &[172])).unwrap_err();
    assert_eq!(
        err,
        ReadError::EventOverrun {
            offset: EV0,
            id: 172,
            needed: 1,
            available: 0
        }
    );
}

#[test]
fn var_payload_overrun_reports_offset() {
    let err = FlpFile::parse(&flp(0, &[9, 1, 200, 5, b'a', b'b'])).unwrap_err();
    assert_eq!(
        err,
        ReadError::EventOverrun {
            offset: EV0 + 2,
            id: 200,
            needed: 5,
            available: 2
        }
    );
}

#[test]
fn truncated_prefix_reports_offset() {
    let err = FlpFile::parse(&flp(0, &[200, 0x80, 0x80])).unwrap_err();
    assert_eq!(
        err,
        ReadError::PrefixTruncated {
            offset: EV0,
            id: 200
        }
    );
    let err = FlpFile::parse(&flp(0, &[9, 0, 200])).unwrap_err();
    assert_eq!(
        err,
        ReadError::PrefixTruncated {
            offset: EV0 + 2,
            id: 200
        }
    );
}

#[test]
fn overflowing_prefix_reports_offset() {
    let mut stream = vec![200];
    stream.extend([0xFF; 10]);
    stream.push(0x7F);
    let err = FlpFile::parse(&flp(0, &stream)).unwrap_err();
    assert_eq!(
        err,
        ReadError::PrefixOverflow {
            offset: EV0,
            id: 200
        }
    );
}

#[test]
fn truncation_never_panics() {
    let mut full = flp(
        1,
        &[
            199, 3, b'2', b'6', 0, 64, 0, 0, 172, 1, 156, 0xE0, 0x93, 4, 0,
        ],
    );
    full.extend(chunk(b"MORE", &[1, 2, 3, 4]));
    roundtrip(&full);
    for n in 0..full.len() {
        let cut = &full[..n];
        if let Ok(f) = FlpFile::parse(cut) {
            assert_eq!(f.to_bytes().unwrap(), cut);
        }
    }
    // Arbitrary garbage behind a valid chunk header, also without panicking.
    for seed in 0u8..=255 {
        let garbage: Vec<u8> = (0..64u8)
            .map(|i| i.wrapping_mul(seed).wrapping_add(seed))
            .collect();
        if let Ok(f) = FlpFile::parse(&chunk(b"FLdt", &garbage)) {
            assert_eq!(f.to_bytes().unwrap(), chunk(b"FLdt", &garbage));
        }
    }
}

// ---- alignment checks ------------------------------------------------------

#[test]
fn aligned_stream_passes() {
    let bytes = flp(2, &[199, 2, b'9', 0, 64, 0, 0, 0, 1, 64, 1, 0, 0, 0]);
    assert_eq!(check_alignment(&roundtrip(&bytes)), []);
}

#[test]
fn channel_count_mismatch() {
    let bytes = flp(3, &[64, 0, 0, 64, 1, 0]);
    assert_eq!(
        check_alignment(&roundtrip(&bytes)),
        [AlignmentIssue::ChannelCountMismatch {
            header: 3,
            found: 2
        }]
    );
}

#[test]
fn missing_header_is_reported() {
    let bytes = chunk(b"FLdt", &[9, 0]);
    assert_eq!(
        check_alignment(&roundtrip(&bytes)),
        [AlignmentIssue::NoChannelCount]
    );
}

/// "Hello!" as UTF-16LE read one byte off: 0x00 becomes an ID-0 event whose value is the
/// next character.
fn text_as_events(text: &str) -> Vec<u8> {
    text.bytes().flat_map(|c| [0, c]).collect()
}

#[test]
fn text_like_run_is_detected() {
    let mut stream = vec![9, 1];
    stream.extend(text_as_events("Hello!"));
    let issues = check_alignment(&roundtrip(&flp(0, &stream)));
    assert_eq!(
        issues,
        [AlignmentIssue::TextLikeRun {
            offset: EV0 + 2,
            len: 6
        }]
    );
}

#[test]
fn short_or_broken_runs_are_not_flagged() {
    // Five printable ID-0 events, then a non-printable one, then five more.
    let mut stream = text_as_events("Hello");
    stream.extend([0, 1]);
    stream.extend(text_as_events("World"));
    // Ordinary ID-0 values (0/1) in long runs are not text-like.
    stream.extend([0, 0].repeat(10));
    assert_eq!(check_alignment(&roundtrip(&flp(0, &stream))), []);
}

#[test]
fn printable_range_bounds() {
    let mut stream = text_as_events("     ~"); // 0x20 and 0x7E are printable
    stream.extend([9, 0]);
    stream.extend([0, 0x1F].repeat(6));
    stream.extend([9, 0]);
    stream.extend([0, 0x7F].repeat(6));
    let issues = check_alignment(&roundtrip(&flp(0, &stream)));
    assert_eq!(
        issues,
        [AlignmentIssue::TextLikeRun {
            offset: EV0,
            len: 6
        }]
    );
}

#[test]
fn event_172_keeps_following_text_aligned() {
    // Well formed only with 172 as u8. Read as u32, the 172 event would eat
    // [1, 192, 14, 'F'] and the rest of the UTF-16 text would decode as ID-0 events.
    let text: Vec<u8> = "FL 26.1"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut stream = vec![172, 1, 192, text.len() as u8];
    stream.extend(&text);
    stream.extend([64, 0, 0]);
    let f = roundtrip(&flp(1, &stream));
    assert_eq!(check_alignment(&f), []);
    assert_eq!(events(&f).len(), 3);
}
