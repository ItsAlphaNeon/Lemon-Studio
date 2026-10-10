//! Browser behaviour: tabs, tree open/close, selection, scrolling and drag and drop into the project.
// spec: ui/browser.md

use os_model::{ChannelKind, ClipSource};

use crate::app::{Action, App, Drag, Win};
use crate::playlist::{self as pl, PickKind};
use crate::views::browser::{self as view, Item};

/// Mouse travel before a press on an item becomes a drag (spec §3).
const DRAG_START: f32 = 4.0;

impl App {
    /// Browser actions; returns whether `a` was one.
    pub(crate) fn browser_act(&mut self, a: &Action) -> bool {
        let s = &mut self.state;
        match *a {
            Action::BrowserTab(i) => {
                s.browser_tab = i;
                s.browser_scroll = 0.0;
            }
            Action::BrowserCollapse => {
                let tab = format!("{}:", s.browser_tab);
                s.browser_open.retain(|k| !k.starts_with(&tab));
                s.browser_scroll = 0.0;
            }
            Action::BrowserList => {}
            Action::BrowserRow(i) => {
                let Some(row) = view::rows(s).into_iter().nth(i) else { return true };
                s.browser_sel = Some(row.key.clone());
                if row.open.is_some() && !s.browser_open.remove(&row.key) {
                    s.browser_open.insert(row.key.clone());
                }
                match row.item {
                    Item::Pattern(n) => {
                        s.pattern = n;
                    }
                    Item::Channel(c) => {
                        s.selected_channel = c;
                        s.rack_selected = Some(c);
                    }
                    _ => {}
                }
                s.browser_scroll = s.browser_scroll.min(view::max_scroll(s));
                if row.item != Item::None {
                    self.drag = Some(Drag::Browser { item: row.item, label: row.label, m0: self.mouse, started: false });
                }
            }
            _ => return false,
        }
        self.invalidate();
        true
    }

    /// Wheel over the browser list.
    pub(crate) fn browser_wheel(&mut self, dy: f32) {
        let s = &mut self.state;
        s.browser_scroll = (s.browser_scroll - dy * 3.0 * view::ROW_H).clamp(0.0, view::max_scroll(s));
        self.invalidate();
    }

    /// Mouse moved while an item is pressed: start the drag, move the label.
    pub(crate) fn browser_drag_move(&mut self, label: String, m0: (f32, f32), started: bool, item: Item) {
        let (x, y) = self.mouse;
        if !started && (x - m0.0).hypot(y - m0.1) < DRAG_START {
            return;
        }
        if !started {
            self.drag = Some(Drag::Browser { item, label: label.clone(), m0, started: true });
        }
        if let Some((_, ox, oy)) = self.state.browser_drag {
            self.damage_rect(view::ghost_rect(ox, oy));
        }
        self.state.browser_drag = Some((label, x, y));
        self.damage_rect(view::ghost_rect(x, y));
    }

    /// Release: drop the dragged item on whatever is under the mouse (spec §3).
    pub(crate) fn browser_drop(&mut self, item: Item, started: bool) {
        self.state.browser_drag = None;
        self.invalidate();
        if !started {
            return;
        }
        let (x, y) = self.mouse;
        let win = self.top_hit(x, y).and_then(|h| h.win);
        let grid = self.pl_grid().filter(|g| g.contains(x, y) && win == Some(Win::Playlist));
        let alt = self.alt;
        let s = &mut self.state;
        // Snapped playlist position under the mouse.
        let spot = grid.map(|grid| {
            let g = pl::PGeom::new(s, grid);
            (g.track_at(y), pl::snap_down(s, &g, g.tick_at(x), alt))
        });
        match item {
            Item::Pattern(n) => {
                if let Some((track, tick)) = spot {
                    pl::snapshot(s);
                    s.playlist.source = Some(ClipSource::Pattern(n));
                    s.playlist.picker = PickKind::Pattern;
                    let i = pl::place(s, track, tick);
                    s.playlist.sel = vec![i];
                } else if matches!(win, Some(Win::PianoRoll | Win::ChannelRack)) {
                    s.pattern = n;
                }
            }
            Item::Channel(c) => {
                let Some(ch) = s.project.channels.get(c) else { return };
                let (kind, index) = (ch.kind, ch.index);
                if let Some((track, tick)) = spot {
                    if matches!(kind, ChannelKind::AudioClip | ChannelKind::Automation) {
                        pl::snapshot(s);
                        s.playlist.source = Some(ClipSource::Channel(index));
                        s.playlist.picker = if kind == ChannelKind::AudioClip { PickKind::Audio } else { PickKind::Automation };
                        let i = pl::place(s, track, tick);
                        s.playlist.sel = vec![i];
                    } else {
                        s.message = Some("Only audio and automation channels go on the playlist; drop a pattern instead".into());
                    }
                } else if matches!(win, Some(Win::PianoRoll | Win::ChannelRack)) {
                    s.selected_channel = c;
                    s.rack_selected = Some(c);
                }
            }
            Item::Sample(path) => {
                if let Some((track, tick)) = spot {
                    let name = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_string();
                    s.message = Some(match pl::add_audio_clip(s, &path, track, tick) {
                        Some(_) => format!("Added audio clip {name}"),
                        None => format!("Could not read {name} as audio"),
                    });
                } else if win == Some(Win::ChannelRack) {
                    let row = crate::app::add_channel(&mut s.project);
                    let ch = &mut s.project.channels[row];
                    ch.name = std::path::Path::new(&path).file_stem().map(|n| n.to_string_lossy().into_owned());
                    ch.sample_path = Some(path);
                    s.selected_channel = row;
                    s.rack_selected = Some(row);
                    s.piano.dirty = true;
                }
            }
            Item::Generator(i) => {
                if spot.is_some() || win == Some(Win::ChannelRack) {
                    self.act(Action::PickerTile(false, i));
                }
            }
            Item::Effect(_) => {
                if win == Some(Win::Mixer) {
                    s.message = Some("Effect placeholder: adding effects to mixer slots is not available yet".into());
                }
            }
            Item::None => {}
        }
    }
}
