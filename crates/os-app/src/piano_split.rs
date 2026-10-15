// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! The splitter between the piano roll's note grid and its event panel (spec: ui/event-panel-splitter.md §2): the
//! share of the height under the ruler that goes to the grid, kept as a fraction so it follows window resizes, with
//! FL's minimums and collapsing, and a double click that hides / brings back the event panel.

/// The band's height (spec: ui/piano-roll.md §1).
pub const BAND: f32 = 8.0;
/// An open event panel / grid is at least this tall; less than half of it collapses the part.
pub const PANEL_MIN: f32 = 45.0;
pub const GRID_MIN: f32 = 46.0;
/// The default layout's event panel (the measured maximized layout, band at bottom − 185).
pub const DEFAULT_PANEL: f32 = 177.0;

/// `f` = the grid's share of the height shared by the grid and the event panel (1 = the panel collapsed, 0 = the grid
/// collapsed); `spare` = what a double click on a collapsed panel brings back. `None` = the default layout.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Split {
    pub f: Option<f32>,
    pub spare: Option<f32>,
}

impl Split {
    /// The grid's height when `h` px (the band excluded) are shared by the grid and the panel.
    pub fn grid_h(self, h: f32) -> f32 {
        let h = h.max(0.0);
        let g = match self.f {
            Some(f) if f >= 1.0 => return h,
            Some(f) if f <= 0.0 => return 0.0,
            Some(f) => (f * h).round(),
            None => h - DEFAULT_PANEL,
        };
        // The minimums of open parts; when there is not room for both, the panel keeps its own.
        g.max(GRID_MIN).min(h - PANEL_MIN).max(0.0)
    }

    /// The split for the band dragged so that the grid would be `grid` px tall (of `h`).
    pub fn dragged(self, grid: f32, h: f32) -> Split {
        let f = if h - grid < PANEL_MIN / 2.0 {
            1.0
        } else if grid < GRID_MIN / 2.0 {
            0.0
        } else {
            grid.max(GRID_MIN).min(h - PANEL_MIN).max(0.0) / h.max(1.0)
        };
        Split { f: Some(f), ..self }
    }

    pub fn panel_collapsed(self) -> bool {
        self.f.is_some_and(|f| f >= 1.0)
    }

    /// Double click: an open event panel collapses (its split kept as the spare); a collapsed one comes back at the
    /// spare split (the default when that is collapsed too).
    pub fn toggled(self) -> Split {
        if self.panel_collapsed() {
            Split { f: self.spare.filter(|&s| s < 1.0), spare: self.spare }
        } else {
            Split { f: Some(1.0), spare: self.f }
        }
    }
}

/// `pianoroll.txt` in the user folder: `split=` and `spare=` (0..1; anything else, or missing, = the default).
fn parse(text: &str) -> Split {
    let mut s = Split::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim().parse::<f32>().ok().filter(|v| (0.0..=1.0).contains(v));
        match k.trim() {
            "split" => s.f = v,
            "spare" => s.spare = v,
            _ => {}
        }
    }
    s
}

fn text(s: Split) -> String {
    let mut t = String::new();
    for (k, v) in [("split", s.f), ("spare", s.spare)] {
        if let Some(v) = v {
            t.push_str(&format!("{k}={v}\n"));
        }
    }
    t
}

/// The saved split (tests: the default).
pub fn load() -> Split {
    #[cfg(test)]
    return Split::default();
    #[cfg(not(test))]
    os_paths::user_file("pianoroll.txt").and_then(|f| std::fs::read_to_string(f).ok()).map(|t| parse(&t)).unwrap_or_default()
}

/// Keeps the split for the next run (tests: nothing is written).
pub fn save(s: Split) {
    #[cfg(not(test))]
    if let Some(f) = os_paths::user_file("pianoroll.txt") {
        let _ = std::fs::write(f, text(s));
    }
    #[cfg(test)]
    let _ = text(s);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimums, collapsing below half of them, fractions over resizes and the double click (spec §1, §2).
    #[test]
    fn split_rules() {
        let h = 700.0;
        assert_eq!(Split::default().grid_h(h), h - DEFAULT_PANEL, "the default keeps a 177 px panel");
        let s = Split::default().dragged(500.0, h);
        assert_eq!(s.grid_h(h), 500.0);
        // The fraction is kept when the window grows; the panel keeps its minimum when it shrinks.
        assert_eq!(s.grid_h(h + 100.0), (500.0f32 / 700.0 * 800.0).round());
        assert_eq!(s.grid_h(500.0), 357.0);
        assert_eq!(Split::default().dragged(650.0, h).grid_h(400.0), 400.0 - PANEL_MIN);
        // Near the bottom: 45 px of panel, then collapsed below 22.5 px; the same for the grid at the top.
        assert_eq!(s.dragged(h - 30.0, h).grid_h(h), h - PANEL_MIN);
        assert_eq!(s.dragged(h - 22.0, h).grid_h(h), h);
        assert_eq!(s.dragged(30.0, h).grid_h(h), GRID_MIN);
        assert_eq!(s.dragged(20.0, h).grid_h(h), 0.0);
        // Double click: collapse, then back to the split it had; a collapsed grid's panel collapses too.
        let c = s.toggled();
        assert!(c.panel_collapsed());
        assert_eq!(c.toggled().grid_h(h), 500.0);
        let top = s.dragged(10.0, h);
        assert!(top.toggled().panel_collapsed());
        assert_eq!(top.toggled().toggled().grid_h(h), 0.0);
        // Collapsed by dragging with nothing kept: back to the default.
        assert_eq!(Split::default().dragged(h, h).toggled().grid_h(h), h - DEFAULT_PANEL);
        // Saved and read back.
        assert_eq!(parse(&text(c)), c);
        assert_eq!(parse("split=2\nspare=x\nother=1"), Split::default());
    }
}
