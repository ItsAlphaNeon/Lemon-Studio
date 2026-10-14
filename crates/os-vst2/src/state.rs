//! What a VST 2 instance saves: the layout FL Studio's plugin wrapper keeps for a VST 2 plugin (the state record of the
//! wrapper state, spec: flp/events/plugins.md §4), so a project's VST 2 plugins move between the two programs.
//!
//! As FL Studio 24.2 to 2026 projects show it: a 21-byte header (the 32-bit values -9, then the byte 13, -2, the size of
//! what follows, 0, 0), then the plugin's own chunk (`op::GET_CHUNK`). The header's other fields are written as seen;
//! their meaning is not known. FL Studio's layout for plugins that keep no chunk is not known either: for those, the
//! chunk's place holds Lemon Studio's own parameter list ([`PARAMS_TAG`]).

/// The header fields before the size, as FL Studio writes them.
const HEAD: i32 = -9;
const MARK: u8 = 13;
const KIND: i32 = -2;
/// Header length; the size is at byte 9.
const HEADER: usize = 21;

/// Starts Lemon Studio's parameter list: the program (`i32`), the count (`u32`), then each value (`f32`), little-endian.
pub const PARAMS_TAG: &[u8; 4] = b"LSVP";

/// A VST 2 instance's saved state.
#[derive(Debug, Clone, PartialEq)]
pub enum Saved {
    /// The plugin's own chunk (a plugin with `flags::CHUNKS`).
    Chunk(Vec<u8>),
    /// The current program and every parameter's value (any other plugin).
    Params { program: i32, values: Vec<f32> },
}

impl Saved {
    pub fn to_bytes(&self) -> Vec<u8> {
        let body = match self {
            Saved::Chunk(c) => c.clone(),
            Saved::Params { program, values } => {
                let mut b = PARAMS_TAG.to_vec();
                b.extend(program.to_le_bytes());
                b.extend((values.len() as u32).to_le_bytes());
                values.iter().for_each(|v| b.extend(v.to_le_bytes()));
                b
            }
        };
        let mut out = Vec::with_capacity(HEADER + body.len());
        out.extend(HEAD.to_le_bytes());
        out.push(MARK);
        out.extend(KIND.to_le_bytes());
        out.extend((body.len() as u32).to_le_bytes());
        out.extend([0; 8]);
        out.extend(body);
        out
    }

    /// Reads a saved state. `chunks`: the plugin keeps a chunk (a parameter list is only read for plugins that do not).
    /// `None` when `data` is not this layout.
    pub fn parse(data: &[u8], chunks: bool) -> Option<Saved> {
        let i32_at = |p: usize| Some(i32::from_le_bytes(data.get(p..p + 4)?.try_into().ok()?));
        if i32_at(0)? != HEAD || *data.get(4)? != MARK || i32_at(5)? != KIND {
            return None;
        }
        let size = usize::try_from(u32::from_le_bytes(data.get(9..13)?.try_into().ok()?)).ok()?;
        let body = data.get(HEADER..HEADER.checked_add(size)?)?;
        if chunks {
            return Some(Saved::Chunk(body.to_vec()));
        }
        let rest = body.strip_prefix(PARAMS_TAG)?;
        let program = i32::from_le_bytes(rest.get(0..4)?.try_into().ok()?);
        let count = usize::try_from(u32::from_le_bytes(rest.get(4..8)?.try_into().ok()?)).ok()?;
        let values = rest.get(8..8 + count.checked_mul(4)?)?.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap_or_default())).collect();
        Some(Saved::Params { program, values })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_read_back_with_the_header_fl_studio_writes() {
        let s = Saved::Chunk(b"#zip#\0data".to_vec());
        let b = s.to_bytes();
        assert_eq!(&b[..21], &[0xF7, 0xFF, 0xFF, 0xFF, 13, 0xFE, 0xFF, 0xFF, 0xFF, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&b[21..], b"#zip#\0data");
        assert_eq!(Saved::parse(&b, true), Some(s));
        // A plugin without chunks does not take another plugin's chunk as its parameters.
        assert_eq!(Saved::parse(&b, false), None);
    }

    #[test]
    fn parameter_lists_read_back() {
        let s = Saved::Params { program: 3, values: vec![0.0, 0.25, 1.0] };
        let b = s.to_bytes();
        assert_eq!(&b[21..25], PARAMS_TAG);
        assert_eq!(Saved::parse(&b, false), Some(s));
    }

    #[test]
    fn other_bytes_are_refused() {
        assert_eq!(Saved::parse(b"", true), None);
        assert_eq!(Saved::parse(b"VST3 preset", true), None);
        // A size running past the end.
        let mut b = Saved::Chunk(vec![1, 2, 3]).to_bytes();
        b.truncate(b.len() - 1);
        assert_eq!(Saved::parse(&b, true), None);
        // A list shorter than its count.
        let mut b = Saved::Params { program: 0, values: vec![0.5; 4] }.to_bytes();
        b[29] = 9;
        assert_eq!(Saved::parse(&b, false), None);
    }
}
