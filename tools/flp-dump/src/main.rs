//! flp-dump: print an FLP file's chunks and event stream, then verify the lossless
//! round-trip and the alignment checks.
//!
//! Usage: flp-dump <file.flp>
//! Exit status: 0 = ok, 1 = malformed file or a failed check, 2 = usage / I/O error.

// spec: flp/container.md (event kinds labelled per flp/coverage.md)

use std::io::{self, BufWriter, Write};
use std::process::ExitCode;

use os_flp::{ChunkBody, Event, FlpFile, Value, check_alignment};

/// Payload bytes shown per variable-length event.
const PREVIEW: usize = 16;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [path] = args.as_slice() else {
        eprintln!("usage: flp-dump <file.flp>");
        return ExitCode::from(2);
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
            "{at:#010x}  chunk {}  len {}",
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
