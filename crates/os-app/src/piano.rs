//! Piano roll state and note editing (view geometry, selection, undo).
// spec: ui/piano-roll.md §3, §5, §8; flp/events/patterns.md (note records)

use os_model::{Note, Pattern};
use os_ui::Rect;

use crate::app::State;
use crate::views::piano::{ROW, TOP_KEY};

/// Undo depth (snapshots of one pattern's notes).
const UNDO_DEPTH: usize = 200;

#[derive(Debug, Clone)]
pub struct PianoState {
    /// Time scroll in ticks (left edge of the grid).
    pub scroll_x: f32,
    /// Key scroll in pixels from the top of the highest key's row.
    pub scroll_y: f32,
    /// Horizontal zoom (§3: 96 px per beat in the default floating window).
    pub px_per_beat: f32,
    /// Length of the last note added, moved or resized; 0 = not yet (§8: first note is PPQ/2).
    pub last_len: u32,
    sel: Vec<usize>,
    sel_key: (u16, u16),
    undo: Vec<(u16, Vec<Note>)>,
}

impl Default for PianoState {
    fn default() -> Self {
        PianoState {
            scroll_x: 0.0,
            scroll_y: ((TOP_KEY - 72) as f32) * ROW,
            px_per_beat: 96.0,
            last_len: 0,
            sel: Vec::new(),
            sel_key: (0, 0),
            undo: Vec::new(),
        }
    }
}

impl PianoState {
    /// Selected note indices (into the pattern's notes) for this pattern and channel.
    pub fn selection(&self, pattern: u16, channel: Option<u16>) -> Vec<usize> {
        match channel {
            Some(c) if self.sel_key == (pattern, c) => self.sel.clone(),
            _ => Vec::new(),
        }
    }

    pub fn select(&mut self, pattern: u16, channel: u16, sel: Vec<usize>) {
        self.sel_key = (pattern, channel);
        self.sel = sel;
    }

    pub fn clear_selection(&mut self) {
        self.sel.clear();
    }

    /// Forget everything tied to the previous project.
    pub fn reset(&mut self) {
        *self = PianoState { px_per_beat: self.px_per_beat, ..PianoState::default() };
    }
}

/// Mapping between grid pixels and ticks / keys for one frame.
pub struct Geom {
    pub grid: Rect,
    pub ppt: f32,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub step: u32,
    pub beat: u32,
    pub bar: u32,
}

impl Geom {
    pub fn new(s: &State, grid: Rect) -> Geom {
        let ppq = u32::from(s.project.ppq.max(4));
        Geom {
            grid,
            ppt: s.piano.px_per_beat / ppq as f32,
            scroll_x: s.piano.scroll_x,
            scroll_y: s.piano.scroll_y,
            step: ppq / 4,
            beat: ppq,
            bar: ppq * 4,
        }
    }

    pub fn x_of(&self, tick: f32) -> f32 {
        self.grid.x + (tick - self.scroll_x) * self.ppt
    }

    pub fn tick_at(&self, x: f32) -> f32 {
        self.scroll_x + (x - self.grid.x) / self.ppt
    }

    /// Top of `key`'s row (17 px of fill, then the separator).
    pub fn y_of(&self, key: i32) -> f32 {
        self.grid.y + (TOP_KEY - key) as f32 * ROW - self.scroll_y
    }

    pub fn key_at(&self, y: f32) -> i32 {
        TOP_KEY - ((y - self.grid.y + self.scroll_y) / ROW).floor() as i32
    }

    /// Keys with at least one visible pixel, lowest first.
    pub fn visible_keys(&self) -> (i32, i32) {
        let hi = self.key_at(self.grid.y).min(TOP_KEY);
        let lo = self.key_at(self.grid.bottom() - 0.01).max(0);
        (lo, hi)
    }

    /// A note covers its row plus the separators above and below (§5).
    pub fn note_rect(&self, n: &Note) -> Option<Rect> {
        let key = i32::from(n.key);
        if key > TOP_KEY {
            return None;
        }
        let len = if n.length == 0 { self.step } else { n.length };
        let x = self.x_of(n.position as f32);
        let w = (len as f32 * self.ppt).max(3.0);
        Some(Rect::new(x, self.y_of(key) - 1.0, w, 19.0))
    }

    /// Snap a tick down to the step grid.
    pub fn snap_down(&self, tick: f32) -> u32 {
        let t = tick.max(0.0) as u32;
        t - t % self.step
    }

    /// Snap a tick delta to the nearest step.
    pub fn snap_delta(&self, dt: f32) -> i64 {
        (dt / self.step as f32).round() as i64 * i64::from(self.step)
    }
}

/// Length used for drawing: a plain step (length 0) is shown one step long (OpenStudio choice).
pub fn shown_len(s: &State, n: &Note) -> u32 {
    if n.length == 0 { u32::from(s.project.ppq.max(4)) / 4 } else { n.length }
}

/// Pattern length in ticks: the last note end rounded up to whole bars, at least one bar.
pub fn pattern_length(s: &State, notes: &[Note]) -> u32 {
    let bar = u32::from(s.project.ppq.max(1)) * 4;
    let end = notes.iter().map(|n| n.position + shown_len(s, n)).max().unwrap_or(0);
    end.div_ceil(bar).max(1) * bar
}

/// Top of a velocity lollipop: `velocity / 128` of the panel height (§7).
pub fn velocity_y(panel: Rect, velocity: u8) -> f32 {
    panel.bottom() - f32::from(velocity) / 128.0 * panel.h
}

pub fn velocity_at(panel: Rect, y: f32) -> u8 {
    ((panel.bottom() - y) / panel.h * 128.0).round().clamp(0.0, 128.0) as u8
}

/// The current pattern, created if it has no data yet.
pub fn pattern_mut(s: &mut State) -> &mut Pattern {
    let number = s.pattern;
    match s.project.patterns.iter().position(|p| p.number == number) {
        Some(i) => &mut s.project.patterns[i],
        None => {
            s.project.patterns.push(Pattern { number, ..Pattern::default() });
            s.project.patterns.last_mut().expect("pushed")
        }
    }
}

pub fn notes(s: &State) -> &[Note] {
    s.project.patterns.iter().find(|p| p.number == s.pattern).map_or(&[], |p| &p.notes)
}

/// The channel index (`Note::channel`) the piano roll edits.
pub fn channel(s: &State) -> Option<u16> {
    s.project.channels.get(s.selected_channel).map(|c| c.index)
}

/// Remember the current pattern's notes for undo.
pub fn snapshot(s: &mut State) {
    let number = s.pattern;
    let notes = notes(s).to_vec();
    let u = &mut s.piano.undo;
    if u.len() >= UNDO_DEPTH {
        u.remove(0);
    }
    u.push((number, notes));
}

/// Restore the last snapshot; returns false when there is nothing to undo.
pub fn undo(s: &mut State) -> bool {
    let Some((number, notes)) = s.piano.undo.pop() else { return false };
    s.pattern = number;
    pattern_mut(s).notes = notes;
    s.piano.clear_selection();
    true
}

/// Add a note with the defaults of a step (patterns.md) at `tick`/`key`; returns its index.
pub fn add_note(s: &mut State, channel: u16, tick: u32, key: i32) -> usize {
    let ppq = u32::from(s.project.ppq.max(4));
    let length = if s.piano.last_len > 0 { s.piano.last_len } else { ppq / 2 };
    let note = Note { length, key: key.clamp(0, TOP_KEY) as u16, ..Note::step(channel, tick) };
    let p = pattern_mut(s);
    p.notes.push(note);
    p.notes.len() - 1
}

/// Delete notes by index.
pub fn delete(s: &mut State, mut idx: Vec<usize>) {
    idx.sort_unstable();
    idx.dedup();
    let p = pattern_mut(s);
    for &i in idx.iter().rev() {
        if i < p.notes.len() {
            p.notes.remove(i);
        }
    }
    s.piano.clear_selection();
}

/// Restore storage order (sorted by position, patterns.md) after an edit, keeping the selection.
pub fn finish_edit(s: &mut State) {
    let (pattern, sel_key) = (s.pattern, s.piano.sel_key);
    let p = pattern_mut(s);
    let mut order: Vec<usize> = (0..p.notes.len()).collect();
    order.sort_by_key(|&i| (p.notes[i].position, p.notes[i].channel));
    if order.iter().enumerate().all(|(a, &b)| a == b) {
        return;
    }
    let sorted: Vec<Note> = order.iter().map(|&i| p.notes[i]).collect();
    p.notes = sorted;
    if sel_key.0 == pattern {
        let mut new_index = vec![0; order.len()];
        for (new, &old) in order.iter().enumerate() {
            new_index[old] = new;
        }
        s.piano.sel = s.piano.sel.iter().filter(|&&i| i < new_index.len()).map(|&i| new_index[i]).collect();
    }
}

/// Scroll vertically so the channel's notes (or C5) are centred in a grid `view_h` pixels tall.
pub fn center_keys(s: &mut State, view_h: f32) {
    let ch = channel(s);
    let keys: Vec<i32> = notes(s).iter().filter(|n| Some(n.channel) == ch).map(|n| i32::from(n.key)).collect();
    let center = match (keys.iter().min(), keys.iter().max()) {
        (Some(lo), Some(hi)) => (lo + hi) as f32 / 2.0,
        _ => 60.0,
    };
    let max = (TOP_KEY + 1) as f32 * ROW - view_h;
    s.piano.scroll_y = ((TOP_KEY as f32 - center) * ROW - view_h / 2.0 + ROW / 2.0).clamp(0.0, max.max(0.0)).round();
    s.piano.scroll_x = 0.0;
}

pub fn clamp_scroll(s: &mut State, view_h: f32) {
    let max = ((TOP_KEY + 1) as f32 * ROW - view_h).max(0.0);
    s.piano.scroll_y = s.piano.scroll_y.clamp(0.0, max).round();
    s.piano.scroll_x = s.piano.scroll_x.max(0.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    fn app() -> App {
        let mut a = App::new(None);
        a.state.selected_channel = 4;
        a
    }

    #[test]
    fn add_sort_select_undo() {
        let mut a = app();
        let s = &mut a.state;
        snapshot(s);
        let i = add_note(s, 4, 192, 72);
        assert_eq!(notes(s)[i].length, 48, "first note is PPQ/2 long");
        assert_eq!(notes(s)[i].velocity, 100);
        snapshot(s);
        let j = add_note(s, 4, 0, 60);
        s.piano.select(s.pattern, 4, vec![j]);
        finish_edit(s);
        assert_eq!(notes(s)[0].position, 0, "sorted by position");
        assert_eq!(s.piano.selection(s.pattern, Some(4)), vec![0], "selection follows the sort");
        assert!(undo(s));
        assert_eq!(notes(s).len(), 1);
        assert!(undo(s));
        assert!(notes(s).is_empty());
        assert!(!undo(s));
    }

    #[test]
    fn geometry_round_trips() {
        let a = app();
        let g = Geom::new(&a.state, Rect::new(282.0, 147.0, 1600.0, 700.0));
        for key in [0, 59, 60, 72, 131] {
            let y = g.y_of(key) + 8.0;
            assert_eq!(g.key_at(y), key);
        }
        assert_eq!(g.snap_down(g.tick_at(g.x_of(100.0))), 96);
        assert_eq!(key_name_check(60), "C5");
    }

    fn key_name_check(k: i32) -> String {
        crate::views::piano::key_name(k)
    }
}
