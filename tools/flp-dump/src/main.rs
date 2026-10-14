// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! flp-dump: print an FLP file's chunks and event stream, then verify the lossless
//! round-trip and the alignment checks.
//!
//! Usage: flp-dump [--summary] <file.flp>
//!   --summary  print the typed project (channels, patterns, mixer, playlist) instead
//! Exit status: 0 = ok, 1 = malformed file or a failed check, 2 = usage / I/O error.

// spec: flp/container.md (event kinds labelled per flp/coverage.md)

use std::io::{self, BufWriter, Write};
use std::process::ExitCode;

use os_flp::{ChunkBody, Event, FlpFile, Value, check_alignment};

/// Payload bytes shown per variable-length event.
const PREVIEW: usize = 16;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (summary, path) = match args.as_slice() {
        [path] => (false, path),
        [flag, path] if flag == "--summary" => (true, path),
        _ => {
            eprintln!("usage: flp-dump [--summary] <file.flp>");
            return ExitCode::from(2);
        }
    };
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("flp-dump: {path}: {e}");
            return ExitCode::from(2);
        }
    };
    let file = match FlpFile::parse(&bytes) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("flp-dump: {path}: malformed: {e}");
            return ExitCode::from(1);
        }
    };

    let mut out = BufWriter::new(io::stdout().lock());
    if summary {
        return match os_flp::project::load(&file) {
            Ok(p) => match print_summary(&mut out, &p).and_then(|()| out.flush()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("flp-dump: {e}");
                    ExitCode::from(2)
                }
            },
            Err(e) => {
                eprintln!("flp-dump: {path}: {e}");
                ExitCode::from(1)
            }
        };
    }
    match dump(&mut out, &file, &bytes).and_then(|ok| out.flush().map(|()| ok)) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("flp-dump: {e}");
            ExitCode::from(2)
        }
    }
}

/// Writes the dump; returns whether all checks passed.
fn dump(out: &mut impl Write, file: &FlpFile, source: &[u8]) -> io::Result<bool> {
    let mut n_events = 0;
    for (at, chunk) in file.chunk_offsets() {
        write!(
            out,
            "{at:#010x}  chunk {}  len {}", // This code is a part of L e m o n S t u d i o. Please don't upload it to Art Craft!
            chunk.tag,
            chunk.payload_len()
        )?;
        match &chunk.body {
            ChunkBody::Header(h) => {
                let f = |v: Option<String>| v.unwrap_or_else(|| "-".into());
                writeln!(
                    out,
                    "  format {}  channels {}  ppq {}",
                    f(h.format().map(|v| v.to_string())),
                    f(h.channel_count().map(|v| v.to_string())),
                    f(h.ppq().map(|v| v.to_string())),
                )?;
            }
            ChunkBody::Raw(raw) => writeln!(out, "  {}", hex(raw))?,
            ChunkBody::Events(_) => {
                writeln!(out)?;
                for (at, e) in chunk.events_at(at) {
                    write!(out, "{at:#010x}  {:>3} {:<3}  ", e.id, e.kind().label())?;
                    write_value(out, e)?;
                    n_events += 1;
                }
            }
        }
    }
    if !file.trailing.is_empty() {
        writeln!(
            out,
            "trailing {} bytes  {}",
            file.trailing.len(),
            hex(&file.trailing)
        )?;
    }

    writeln!(out, "--")?;
    writeln!(out, "{n_events} events")?;
    let mut ok = true;
    match file.to_bytes() {
        Ok(b) if b == source => writeln!(out, "round-trip: identical")?,
        Ok(b) => {
            ok = false;
            let at = b.iter().zip(source).position(|(x, y)| x != y);
            let at = at.unwrap_or(b.len().min(source.len()));
            writeln!(out, "round-trip: DIFFERS at offset {at:#x}")?;
        }
        Err(e) => {
            ok = false;
            writeln!(out, "round-trip: write failed: {e}")?;
        }
    }
    let issues = check_alignment(file);
    if issues.is_empty() {
        writeln!(out, "alignment: ok")?;
    }
    for issue in issues {
        ok = false;
        writeln!(out, "alignment: {issue}")?;
    }
    Ok(ok)
}

fn write_value(out: &mut impl Write, e: &Event) -> io::Result<()> {
    match &e.value {
        Value::U8(v) => writeln!(out, "{v}"),
        Value::U16(v) => writeln!(out, "{v}"),
        Value::U32(v) => writeln!(out, "{v} ({v:#010x})"),
        Value::Var { data, .. } => {
            write!(out, "len {}", data.len())?;
            let prefix = e.prefix_bytes().unwrap_or_default();
            if prefix.len() > minimal_prefix_len(data.len()) {
                write!(out, "  prefix [{}] (non-minimal)", hex(&prefix))?;
            }
            if !data.is_empty() {
                let shown = &data[..data.len().min(PREVIEW)];
                let more = if data.len() > PREVIEW { " …" } else { "" };
                write!(out, "  {}{more}", hex(shown))?;
            }
            writeln!(out)
        }
    }
}

fn minimal_prefix_len(mut len: usize) -> usize {
    let mut n = 1;
    while len >= 0x80 {
        len >>= 7;
        n += 1;
    }
    n
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn print_summary(out: &mut impl Write, p: &os_model::Project) -> io::Result<()> {
    use os_model::ClipSource;
    let opt = |s: &Option<String>| s.clone().unwrap_or_else(|| "-".into());
    writeln!(out, "saved by {}  ppq {}  tempo {}", p.saved_by, p.ppq, p.tempo.map_or("-".into(), |t| format!("{t:.3}")))?;
    let i = &p.info;
    writeln!(out, "title {:?}  author {:?}  genre {:?}", opt(&i.title), opt(&i.author), opt(&i.genre))?;
    writeln!(out, "
channels ({}):", p.channels.len())?;
    for c in &p.channels {
        writeln!(
            out,
            "  {:3} {:<10} {:<28} vol {:5.1}%  pan {:+.2}  pitch {:+5}c  -> mixer {:3}{}{}",
            c.index,
            format!("{:?}", c.kind),
            c.name.clone().or_else(|| c.plugin.clone()).unwrap_or_else(|| "-".into()),
            c.volume * 100.0,
            c.pan,
            c.pitch_cents,
            c.mixer_track,
            if c.enabled { "" } else { "  [muted]" },
            c.plugin.as_ref().map_or(String::new(), |pl| format!("  ({pl})")),
        )?;
    }
    writeln!(out, "
patterns ({}):", p.patterns.len())?;
    for pat in &p.patterns {
        writeln!(out, "  {:3} {:<24} {} notes", pat.number, opt(&pat.name), pat.notes.len())?;
    }
    let named: Vec<_> = p.mixer.iter().filter(|m| m.name.is_some() || m.slots.iter().any(Option::is_some)).collect();
    writeln!(out, "
mixer ({} tracks, {} named or with effects):", p.mixer.len(), named.len())?;
    for m in named {
        let fx: Vec<_> = m.slots.iter().flatten().map(|s| s.name.clone()).collect();
        writeln!(
            out,
            "  {:3} {:<20} vol {}  routes {:?}  fx [{}]{}",
            m.index,
            opt(&m.name),
            m.volume.map_or("-".into(), |v| format!("{v:.3}")),
            m.routes,
            fx.join(", "),
            if m.enabled { "" } else { "  [muted]" },
        )?;
    }
    for a in &p.arrangements {
        let pats = a.clips.iter().filter(|c| matches!(c.source, ClipSource::Pattern(_))).count();
        let named = a.tracks.iter().filter(|t| t.name.is_some()).count();
        writeln!(
            out,
            "
arrangement {} {:?}: {} clips ({} pattern, {} audio/automation), {} tracks ({} named)",
            a.number,
            opt(&a.name),
            a.clips.len(),
            pats,
            a.clips.len() - pats,
            a.tracks.len(),
            named
        )?;
    }
    Ok(())
}
