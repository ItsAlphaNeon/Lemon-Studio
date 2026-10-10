//! Popup menus: layout and painting, independent of the window they live in (the app paints them in its frame, plugins
//! in their own popup windows). Two looks, both measured on FL: the grey **context** menu (right-click menus, menu bar
//! dropdowns: headers, separators, ticks, shortcuts, submenu arrows) and the dark **list** menu (choice boxes: the
//! current entry is lit, section headers).
// spec: ui/menus.md (context look), plugins/open-sampler.md §Menus (list look, item lists)

use os_theme::{Color, Palette};

use super::{tick, Dir};
use crate::{Canvas, Font, Rect};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    None,
    Check(bool),
    Radio(bool),
}

/// One row of a menu.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuItem {
    Item { label: String, shortcut: String, mark: Mark, sub: bool, enabled: bool },
    /// Section header (not clickable).
    Head(String),
    Sep,
    /// Starts the next column.
    Column,
}

impl MenuItem {
    pub fn item(label: &str) -> MenuItem {
        MenuItem::Item { label: label.into(), shortcut: String::new(), mark: Mark::None, sub: false, enabled: true }
    }
    pub fn head(label: &str) -> MenuItem {
        MenuItem::Head(label.into())
    }
    pub fn shortcut(self, s: &str) -> MenuItem {
        match self {
            MenuItem::Item { label, mark, sub, enabled, .. } => MenuItem::Item { label, shortcut: s.into(), mark, sub, enabled },
            o => o,
        }
    }
    pub fn mark(self, m: Mark) -> MenuItem {
        match self {
            MenuItem::Item { label, shortcut, sub, enabled, .. } => MenuItem::Item { label, shortcut, mark: m, sub, enabled },
            o => o,
        }
    }
    pub fn sub(self) -> MenuItem {
        match self {
            MenuItem::Item { label, shortcut, mark, enabled, .. } => MenuItem::Item { label, shortcut, mark, sub: true, enabled },
            o => o,
        }
    }
    pub fn enabled(self, on: bool) -> MenuItem {
        match self {
            MenuItem::Item { label, shortcut, mark, sub, .. } => MenuItem::Item { label, shortcut, mark, sub, enabled: on },
            o => o,
        }
    }
    pub fn clickable(&self) -> bool {
        matches!(self, MenuItem::Item { enabled: true, .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuStyle {
    /// Grey right-click menu (ticks column, shortcuts, arrows).
    Context,
    /// Dark choice list: `current` row lit.
    List,
}

pub const ROW: f32 = 19.0;
pub const HEAD: f32 = 16.0;
pub const SEP: f32 = 7.0;
pub const BORDER: f32 = 2.0;

fn font(style: MenuStyle) -> Font {
    match style {
        MenuStyle::Context => Font::new(14),
        MenuStyle::List => Font::new(14),
    }
}

fn row_h(it: &MenuItem) -> f32 {
    match it {
        MenuItem::Item { .. } => ROW,
        MenuItem::Head(_) => HEAD,
        MenuItem::Sep => SEP,
        MenuItem::Column => 0.0,
    }
}

/// Where everything of a menu sits (menu-local coordinates: the panel's top-left is 0, 0).
#[derive(Debug, Clone, Default)]
pub struct MenuLayout {
    pub w: f32,
    pub h: f32,
    /// Item index -> row rect, for every row (headers and separators included).
    pub rows: Vec<Rect>,
}

impl MenuLayout {
    /// The clickable item under a point (menu-local).
    pub fn item_at(&self, items: &[MenuItem], x: f32, y: f32) -> Option<usize> {
        self.rows.iter().enumerate().find(|(i, r)| r.contains(x, y) && matches!(items[*i], MenuItem::Item { .. })).map(|(i, _)| i)
    }
}

/// Lays a menu out: columns as wide as their widest row, rows stacked.
pub fn layout(c: &mut Canvas, items: &[MenuItem], style: MenuStyle) -> MenuLayout {
    let f = font(style);
    let mut cols: Vec<Vec<usize>> = vec![Vec::new()];
    for (i, it) in items.iter().enumerate() {
        if matches!(it, MenuItem::Column) {
            cols.push(Vec::new());
        } else {
            cols.last_mut().unwrap().push(i);
        }
    }
    let mut rows = vec![Rect::default(); items.len()];
    let mut x = BORDER;
    let mut h: f32 = 0.0;
    for col in &cols {
        let mut w: f32 = 60.0;
        for &i in col {
            w = w.max(match &items[i] {
                MenuItem::Item { label, shortcut, sub, .. } => {
                    let lead = if style == MenuStyle::Context { 20.0 } else { 6.0 };
                    let sc = if shortcut.is_empty() { 0.0 } else { c.text_width(f.italic(), shortcut) + 24.0 };
                    lead + c.text_width(f, label) + sc + if *sub { 26.0 } else { 14.0 }
                }
                MenuItem::Head(s) => c.text_width(Font::new(12), s) + 16.0,
                _ => 0.0,
            });
        }
        let w = w.ceil();
        let mut y = BORDER;
        for &i in col {
            let rh = row_h(&items[i]);
            rows[i] = Rect::new(x, y, w, rh);
            y += rh;
        }
        h = h.max(y - BORDER);
        x += w;
    }
    MenuLayout { w: x + BORDER, h: h + 2.0 * BORDER, rows }
}

/// Paints a menu at (`ox`, `oy`) (the layout's origin). `hover` = row under the mouse, `current` = lit row of a list
/// menu (the chosen entry) shown when nothing is hovered.
pub fn paint(c: &mut Canvas, t: &Palette, items: &[MenuItem], l: &MenuLayout, style: MenuStyle, ox: f32, oy: f32, hover: Option<usize>, current: Option<usize>) {
    let r = Rect::new(ox, oy, l.w, l.h);
    let f = font(style);
    let lh = c.line_height(f);
    let (body, border, hi, text, dim, head, head_text, sc) = match style {
        MenuStyle::Context => (t.menu_pop, t.menu_pop_border, t.menu_pop_hi, t.menu_pop_text, t.menu_pop_dim, t.menu_pop_head, t.menu_pop_head_text, t.menu_pop_shortcut),
        MenuStyle::List => (t.s(0x1d1e1f), t.s(0x101112), t.s(0x89949b), t.tx(0xc5ccd0), t.s(0x6a6e70), t.s(0x303131), t.s(0x7e8486), t.s(0x9ea4a3)),
    };
    c.fill_rect(r, border);
    c.fill_rect(r.inset(BORDER), body);
    // Column dividers.
    let mut last_x = None;
    for (i, row) in l.rows.iter().enumerate() {
        if matches!(items[i], MenuItem::Column) {
            continue;
        }
        if let Some(px) = last_x {
            if row.x > px {
                c.fill_rect(Rect::new(ox + row.x - 1.0, oy + BORDER, 1.0, l.h - 2.0 * BORDER), head);
            }
        }
        last_x = Some(row.x);
    }
    let lit = hover.or(current);
    for (i, it) in items.iter().enumerate() {
        let rr = Rect::new(ox + l.rows[i].x, oy + l.rows[i].y, l.rows[i].w, l.rows[i].h);
        match it {
            MenuItem::Column => {}
            MenuItem::Sep => c.fill_rect(Rect::new(rr.x, rr.y + 3.0, rr.w, 1.0), head),
            MenuItem::Head(s) => {
                c.fill_rect(rr, head);
                c.text(Font::new(12), rr.x + 4.0, rr.y + 1.0, head_text, s);
            }
            MenuItem::Item { label, shortcut, mark, sub, enabled } => {
                let on = lit == Some(i) && (*enabled || style == MenuStyle::List);
                if on {
                    c.fill_rect(rr, hi);
                }
                let col = if !enabled {
                    dim
                } else if on && style == MenuStyle::List {
                    t.tx(0xffffff)
                } else {
                    text
                };
                let (mx, my) = (rr.x + 10.0, rr.cy());
                match mark {
                    Mark::Check(true) => tick(c, Rect::new(mx - 6.0, my - 6.0, 12.0, 12.0), if *enabled { text } else { dim }),
                    Mark::Check(false) => tick(c, Rect::new(mx - 6.0, my - 6.0, 12.0, 12.0), text.with_alpha(40)),
                    Mark::Radio(true) => c.fill_circle(mx, my, 3.0, text),
                    Mark::Radio(false) => c.stroke_circle(mx, my, 3.5, 1.0, dim),
                    Mark::None => {}
                }
                let lead = if style == MenuStyle::Context { 20.0 } else { 6.0 };
                c.text_clip(f, rr.x + lead, (rr.cy() - lh / 2.0).round(), rr.w - lead - 6.0, col, label);
                let right = rr.right() - if *sub { 16.0 } else { 8.0 };
                if !shortcut.is_empty() {
                    c.text_right(f.italic(), right, (rr.cy() - lh / 2.0).round() + 1.0, if *enabled { sc } else { dim }, shortcut);
                }
                if *sub {
                    super::chevron(c, rr.right() - 9.0, my, 3.5, Dir::Right, if *enabled { text } else { dim });
                }
            }
        }
    }
}

/// A menu's background colour (for windows that host it).
pub fn body(t: &Palette, style: MenuStyle) -> Color {
    match style {
        MenuStyle::Context => t.menu_pop,
        MenuStyle::List => t.s(0x1d1e1f),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_and_hit_test() {
        let mut c = Canvas::new(400, 300, 1.0).unwrap();
        let items = vec![
            MenuItem::head("Realtime"),
            MenuItem::item("Resample"),
            MenuItem::item("Stretch"),
            MenuItem::Column,
            MenuItem::item("Auto"),
        ];
        let l = layout(&mut c, &items, MenuStyle::List);
        assert_eq!(l.rows[1].y, BORDER + HEAD);
        assert!(l.rows[4].x > l.rows[1].x);
        assert_eq!(l.h, HEAD + 2.0 * ROW + 2.0 * BORDER);
        assert_eq!(l.item_at(&items, l.rows[2].cx(), l.rows[2].cy()), Some(2));
        assert_eq!(l.item_at(&items, l.rows[0].cx(), l.rows[0].cy()), None);
        let t = Palette::default();
        paint(&mut c, &t, &items, &l, MenuStyle::List, 0.0, 0.0, None, Some(1));
        paint(&mut c, &t, &items, &l, MenuStyle::Context, 0.0, 0.0, Some(2), None);
    }
}
