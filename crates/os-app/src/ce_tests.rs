// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Cutting-edge UI tests for [`App`]: in-app plugin cards & tabs, tiling layout & split gaps, piano-roll off-view
//! arrows & stacked notes, and Zest embedded rendering. Kept out of `app.rs` so merges from `main` stay clean.

use super::tests::{assert_same, click, frame, frame_damage, hit_rect};
use super::*;

/// Two windows of one plugin apart: one dragged onto the other and held shows the group in its own row of tabs, the
/// other unchanged; moved away the row goes, and held again and let go they are one window.
#[cfg(lemon_cards)]
fn tabs_go_back_together(app: &mut App, c: &mut Canvas) {
    use std::time::{Duration, Instant};
    let (e0, e1) = (app.state.embeds[&0].clone(), app.state.embeds[&1].clone());
    e1.set_pos((e0.rect().x + 40.0, e0.rect().y + 300.0));
    app.plugin_tick();
    frame(app, c, None);
    let (r0, r1) = (e0.rect(), e1.rect());
    let (title, over) = ((r1.x + 150.0, r1.y + 8.0), (r0.x + 150.0, r0.y + 8.0));
    app.mouse_move(title.0, title.1);
    assert!(app.embed_down(os_plugkit::input::Button::Left));
    app.mouse_move(over.0, over.1);
    assert!(e1.moving(), "its title moves it");
    assert!(app.tab_held.is_some_and(|h| h.into == 0 && !h.ripe) && app.animating(), "the hold has begun");
    app.tab_hold_tick(Instant::now());
    assert!(app.state.tab_preview.is_none(), "not yet");
    app.tab_hold_tick(Instant::now() + Duration::from_millis(400));
    assert_eq!(app.state.tab_preview, Some(crate::plugin_tabs::Preview { row: 1, rows: vec![0, 1] }));
    app.plugin_tick();
    assert!(e1.has_tabs() && e1.rect().h == r1.h, "the held window has the row, on top of its frame");
    assert_eq!(e0.rect().h, r0.h, "the one below is as it was");
    frame(app, c, None);
    // Well inside the other window, below its title, is no place to join.
    app.mouse_move(over.0, r0.y + r0.h / 2.0);
    assert!(app.state.tab_preview.is_none() && app.tab_held.is_none(), "too low to join");
    app.plugin_tick();
    // Just below the title still is: the dragged window hides it.
    app.mouse_move(over.0, e0.head_rect().bottom() + 12.0);
    app.tab_hold_tick(Instant::now() + Duration::from_millis(400));
    assert!(app.state.tab_preview.is_some(), "a little below the title joins");
    // Away: it goes, and letting go there leaves them apart.
    let away = if r0.right() + 60.0 < 1536.0 { (r0.right() + 40.0, over.1) } else { (r0.x - 40.0, over.1) };
    app.mouse_move(away.0, away.1);
    assert!(app.state.tab_preview.is_none() && app.tab_held.is_none());
    app.plugin_tick();
    assert_eq!(e1.rect().h, r1.h);
    // Back and held, let go: one window, where it was let go.
    app.mouse_move(over.0, over.1);
    app.tab_hold_tick(Instant::now() + Duration::from_millis(400));
    let dropped = (e1.rect().x, e1.rect().y);
    assert!(app.embed_up());
    assert!(app.state.tab_preview.is_none());
    app.plugin_tick();
    assert_eq!(app.state.plugin_tabs.group_of(0).map(|g| (g.rows.clone(), g.active)), Some((vec![0, 1], 1)));
    assert_eq!((e1.rect().x, e1.rect().y), dropped, "it stays where it was let go");
    assert_eq!((e0.rect().x, e0.rect().y), dropped, "the other comes to it");
    assert!(!app.state.z.contains(&Win::Hosted(0)) && app.state.z.contains(&Win::Hosted(1)));
    // Pulled out for what follows.
    app.state.plugin_tabs.pull_out(1);
    app.plugin_tick();
}

/// As `tabs_go_back_together`, through the shell's own calls and clock: presses, motion in steps, the hold waited
/// out in real time with frames between, the release. Then a tab dragged out of the group and, in the same drag,
/// held over the window it came from goes back.
#[cfg(lemon_cards)]
fn tabs_rejoin_as_the_shell_drives_it(app: &mut App, c: &mut Canvas) {
    use std::time::Duration;
    let (e0, e1) = (app.state.embeds[&0].clone(), app.state.embeds[&1].clone());
    let wait = |app: &mut App, c: &mut Canvas| {
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(20));
            app.tick();
            app.plugin_tick();
            let d = app.take_damage();
            frame_damage(app, c, d);
        }
    };
    let walk = |app: &mut App, from: (f32, f32), to: (f32, f32)| {
        for i in 1..=10 {
            let k = i as f32 / 10.0;
            app.mouse_move(from.0 + (to.0 - from.0) * k, from.1 + (to.1 - from.1) * k);
        }
    };
    e1.set_pos((e0.rect().x + 40.0, e0.rect().y + 300.0));
    app.state.raise(Win::Hosted(1));
    app.plugin_tick();
    frame(app, c, None);
    let (r0, r1) = (e0.rect(), e1.rect());
    // (not where the last test pressed: that would be a double click)
    let (title, over) = ((r1.x + 220.0, r1.y + 8.0), (r0.x + 220.0, r0.y + 8.0));
    app.mouse_move(title.0, title.1);
    app.mouse_down();
    walk(app, title, over);
    wait(app, c);
    assert_eq!(app.state.tab_preview.as_ref().map(|p| p.row), Some(1), "held over the other, it shows the group");
    app.mouse_up();
    app.plugin_tick();
    assert_eq!(app.state.plugin_tabs.group_of(0).map(|g| g.rows.clone()), Some(vec![0, 1]), "let go: one window");
    frame(app, c, None);
    // A tab dragged out, then back over the window it left and held there.
    let strip = e1.tabs_rect();
    let tab = (strip.x + 20.0, strip.y + 10.0);
    app.mouse_move(tab.0, tab.1);
    app.mouse_down();
    walk(app, tab, (tab.0 + 300.0, tab.1 + 250.0));
    app.plugin_tick();
    assert!(app.state.plugin_tabs.group_of(1).is_none(), "out of the group");
    let back = (e1.rect().x + 150.0, e1.rect().y + 8.0);
    walk(app, (tab.0 + 300.0, tab.1 + 250.0), back);
    wait(app, c);
    assert!(app.state.tab_preview.is_some(), "held over the window it came from, it shows the group");
    app.mouse_up();
    app.plugin_tick();
    assert!(app.state.plugin_tabs.group_of(0).is_some_and(|g| g.rows.len() == 2), "back together");
    app.state.plugin_tabs.pull_out(1);
    app.plugin_tick();
}

/// Two windows of one plugin are one window with tabs; a tab shows on a press, and a drag takes it out.
#[cfg(lemon_cards)]
#[test]
fn windows_of_one_plugin_are_tabs() {
    use crate::plugin_window::ChannelInfo;
    use std::time::{Duration, Instant};
    let Ok(mut c) = Canvas::new(1536, 864, 1.0) else { return };
    let mut app = App::new(None);
    app.state.session.plugins.available = crate::plugins::builtin();
    app.state.session.plugins.scale = 1.0;
    app.state.session.project.get_mut().channels.push(os_model::Channel::default());
    frame(&mut app, &mut c, None);
    let index = app.state.session.plugins.index_of(crate::citrus::CITRUS).expect("built-in Citrus");
    let info = |name: &str| ChannelInfo { icon: None, name: name.into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None };
    let settle = |app: &mut App, rows: &[usize]| {
        let t0 = Instant::now();
        while !rows.iter().all(|r| app.state.embeds.get(r).is_some_and(|e| e.claimed())) && t0.elapsed() < Duration::from_secs(3) {
            app.plugin_tick();
            std::thread::sleep(Duration::from_millis(10));
        }
        app.plugin_tick();
    };
    app.state.session.plugins.create_with(0, index, None).unwrap();
    app.state.editor_host.open_window(&mut app.state.session.plugins, 0, info("One")).unwrap();
    settle(&mut app, &[0]);
    let alone = app.state.embeds[&0].rect();
    app.state.session.plugins.create_with(1, index, None).unwrap();
    app.state.editor_host.open_window(&mut app.state.session.plugins, 1, info("Two")).unwrap();
    settle(&mut app, &[0, 1]);
    app.plugin_tick();
    assert!(app.state.plugin_tabs.hidden(0) && !app.state.plugin_tabs.hidden(1), "the new one shows");
    assert!(!app.state.z.contains(&Win::Hosted(0)) && app.state.z.contains(&Win::Hosted(1)));
    let (a, b) = (app.state.embeds[&0].rect(), app.state.embeds[&1].rect());
    assert_eq!((a.x, a.y), (b.x, b.y), "one place");
    assert_eq!((b.w, b.h), (alone.w, alone.h), "the frame keeps its size: no row is added to it");
    let row = app.state.embeds[&1].tabs_rect();
    assert_eq!((row.bottom(), row.h), (b.y + 1.0, 26.0), "the tabs sit on top of the frame, over its top edge");
    assert!(row.x > b.x && row.right() < b.right(), "clear of the frame's rounded corners: {row:?} on {b:?}");
    frame(&mut app, &mut c, None);
    let strip = app.state.embeds[&1].tabs_rect();
    // A press on the first tab shows it.
    let first = (strip.x + 20.0, strip.y + 10.0);
    app.mouse_move(first.0, first.1);
    if let Some(Some(rects)) = app.take_damage() {
        let whole = |r: &Rect| r.x <= strip.x && r.right() >= strip.right() && r.y <= strip.y && r.bottom() >= strip.bottom();
        assert!(rects.iter().any(whole), "a hover repaints the whole row: {rects:?} {strip:?}");
    }
    assert!(matches!(app.top_hit(first.0, first.1).map(|h| h.action.clone()), Some(Action::PluginTab(0))));
    assert!(app.embed_down(os_plugkit::input::Button::Left));
    assert!(app.embed_up());
    app.plugin_tick();
    assert!(!app.state.plugin_tabs.hidden(0) && app.state.plugin_tabs.hidden(1));
    frame(&mut app, &mut c, None);
    // Dragged out, it stands alone where the mouse goes.
    app.mouse_move(first.0, first.1);
    assert!(app.embed_down(os_plugkit::input::Button::Left));
    app.mouse_move(first.0 + 300.0, first.1 + 200.0);
    assert!(app.embed_up());
    app.plugin_tick();
    assert!(app.state.plugin_tabs.group_of(0).is_none() && app.state.plugin_tabs.group_of(1).is_none());
    assert!(app.state.z.contains(&Win::Hosted(0)) && app.state.z.contains(&Win::Hosted(1)), "both show");
    let (a, b) = (app.state.embeds[&0].rect(), app.state.embeds[&1].rect());
    assert_eq!(b.x, a.x - 300.0, "apart");
    assert_eq!(a.h, alone.h, "no row of tabs");
    tabs_go_back_together(&mut app, &mut c);
    tabs_rejoin_as_the_shell_drives_it(&mut app, &mut c);
    app.state.editor_host.close_window(&mut app.state.session.plugins, 0);
    app.state.editor_host.close_window(&mut app.state.session.plugins, 1);
}

/// The selected channel changing (Select up/down, a pad, the rack) to one whose window is a tab not showing shows
/// that tab, in the window's place: a window in front of it stays in front. A channel with no window changes nothing.
#[cfg(lemon_cards)]
#[test]
fn the_tab_showing_follows_the_selected_channel() {
    use crate::plugin_window::ChannelInfo;
    use std::time::{Duration, Instant};
    let Ok(mut c) = Canvas::new(1536, 864, 1.0) else { return };
    let mut app = App::new(None);
    app.state.session.plugins.available = crate::plugins::builtin();
    app.state.session.plugins.scale = 1.0;
    for _ in 0..2 {
        app.state.session.project.get_mut().channels.push(os_model::Channel::default());
    }
    frame(&mut app, &mut c, None);
    let index = app.state.session.plugins.index_of(crate::citrus::CITRUS).expect("built-in Citrus");
    let info = |name: &str| ChannelInfo { icon: None, name: name.into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None };
    for (row, name) in [(0, "One"), (1, "Two")] {
        app.state.session.plugins.create_with(row, index, None).unwrap();
        app.state.editor_host.open_window(&mut app.state.session.plugins, row, info(name)).unwrap();
    }
    let t0 = Instant::now();
    while ![0, 1].iter().all(|r| app.state.embeds.get(r).is_some_and(|e| e.claimed())) && t0.elapsed() < Duration::from_secs(3) {
        app.plugin_tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    app.state.selected_channel = 1;
    app.plugin_tick();
    assert!(app.state.plugin_tabs.hidden(0) && !app.state.plugin_tabs.hidden(1), "the newer one shows");
    // Another window in front of the plugin window.
    app.state.raise(Win::Mixer);
    let at = |app: &App, w: Win| app.state.z.iter().position(|&o| o == w);
    let place = at(&app, Win::Hosted(1)).expect("in the stack");
    app.state.selected_channel = 0;
    app.plugin_tick();
    assert!(!app.state.plugin_tabs.hidden(0) && app.state.plugin_tabs.hidden(1), "the selected channel's tab shows");
    assert_eq!(at(&app, Win::Hosted(0)), Some(place), "in the window's place");
    assert_eq!(app.state.z.last(), Some(&Win::Mixer), "not raised");
    assert_eq!(at(&app, Win::Hosted(1)), None);
    // A channel with no window: the tab showing stays.
    app.state.selected_channel = 2;
    app.plugin_tick();
    assert!(!app.state.plugin_tabs.hidden(0) && app.state.plugin_tabs.hidden(1));
    app.state.selected_channel = 1;
    app.plugin_tick();
    assert!(app.state.plugin_tabs.hidden(0) && !app.state.plugin_tabs.hidden(1), "and back");
    app.state.editor_host.close_window(&mut app.state.session.plugins, 0);
    app.state.editor_host.close_window(&mut app.state.session.plugins, 1);
}

/// The tab showing closed from the instrument rack (its window button clicked again), over and over: the window
/// left is in the stack and painted at the very next tick, every time.
#[cfg(lemon_cards)]
#[test]
fn a_tab_closed_from_the_rack_leaves_the_other_showing() {
    use crate::plugin_window::ChannelInfo;
    use std::time::{Duration, Instant};
    let Ok(mut c) = Canvas::new(1536, 864, 1.0) else { return };
    let mut app = App::new(None);
    app.state.session.plugins.available = crate::plugins::builtin();
    app.state.session.plugins.scale = 1.0;
    app.state.session.project.get_mut().channels.push(os_model::Channel::default());
    frame(&mut app, &mut c, None);
    let index = app.state.session.plugins.index_of(crate::citrus::CITRUS).expect("built-in Citrus");
    let info = |name: &str| ChannelInfo { icon: None, name: name.into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None };
    let settle = |app: &mut App, rows: &[usize]| {
        let t0 = Instant::now();
        while !rows.iter().all(|r| app.state.embeds.get(r).is_some_and(|e| e.claimed())) && t0.elapsed() < Duration::from_secs(3) {
            app.plugin_tick();
            std::thread::sleep(Duration::from_millis(10));
        }
        app.plugin_tick();
    };
    app.state.session.plugins.create_with(0, index, None).unwrap();
    app.state.editor_host.open_window(&mut app.state.session.plugins, 0, info("Vangelis")).unwrap();
    settle(&mut app, &[0]);
    app.state.session.plugins.create_with(1, index, None).unwrap();
    for round in 0..3 {
        app.state.editor_host.open_window(&mut app.state.session.plugins, 1, info("Hydronium")).unwrap();
        settle(&mut app, &[0, 1]);
        app.plugin_tick();
        assert!(app.state.plugin_tabs.hidden(0) && app.state.z.contains(&Win::Hosted(1)), "round {round}: the new tab shows");
        frame(&mut app, &mut c, None);
        assert!(app.state.editor_host.close_if_open(&mut app.state.session.plugins, 1));
        app.plugin_tick();
        assert!(app.state.z.contains(&Win::Hosted(0)), "round {round}: the one left is in the stack at once");
        assert!(!app.state.plugin_tabs.hidden(0) && app.state.plugin_tabs.group_of(0).is_none());
        frame(&mut app, &mut c, None);
    }
    app.state.editor_host.close_window(&mut app.state.session.plugins, 0);
}

/// A tab's little x closes that tab; the window's close closes them all, in either layout.
#[cfg(lemon_cards)]
#[test]
fn a_tab_closes_alone_and_the_window_closes_all() {
    use crate::plugin_window::ChannelInfo;
    use std::time::{Duration, Instant};
    let Ok(mut c) = Canvas::new(1536, 864, 1.0) else { return };
    let mut app = App::new(None);
    app.state.session.plugins.available = crate::plugins::builtin();
    app.state.session.plugins.scale = 1.0;
    app.state.session.project.get_mut().channels.extend([os_model::Channel::default(), os_model::Channel::default()]);
    frame(&mut app, &mut c, None);
    let index = app.state.session.plugins.index_of(crate::citrus::CITRUS).expect("built-in Citrus");
    let info = |name: &str| ChannelInfo { icon: None, name: name.into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None };
    let rows = [0, 1, 2];
    for r in rows {
        app.state.session.plugins.create_with(r, index, None).unwrap();
        app.state.editor_host.open_window(&mut app.state.session.plugins, r, info("Tab")).unwrap();
    }
    let t0 = Instant::now();
    while !rows.iter().all(|r| app.state.embeds.get(r).is_some_and(|e| e.claimed())) && t0.elapsed() < Duration::from_secs(3) {
        app.plugin_tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    app.plugin_tick();
    assert_eq!(app.state.plugin_tabs.group_of(0).map(|g| g.rows.len()), Some(3));
    frame(&mut app, &mut c, None);
    // The little x of the first tab, not showing: it goes, and the one showing stays.
    let strip = app.state.embeds[&2].tabs_rect();
    let close = crate::plugin_tabs::row_shapes(strip, 3, 2, crate::prefs::check(&crate::prefs::PLUGIN_TABS_STACKED)).into_iter().find(|t| t.index == 0).and_then(|t| t.close()).unwrap();
    let at = (close.cx(), close.cy());
    app.mouse_move(at.0, at.1);
    assert_eq!(app.top_hit(at.0, at.1).map(|h| h.action.clone()), Some(Action::PluginTabClose(0)));
    assert!(app.embed_down(os_plugkit::input::Button::Left));
    assert!(app.embed_up());
    app.plugin_tick();
    assert!(!app.state.embeds.contains_key(&0), "that tab closed");
    assert!(app.state.embeds.contains_key(&1) && app.state.embeds.contains_key(&2), "the others stay");
    assert_eq!(app.state.plugin_tabs.group_of(1).map(|g| (g.rows.len(), g.active)), Some((2, 2)));
    frame(&mut app, &mut c, None);
    // The little x of the one showing: the other shows at once, while the plugin is still closing this one.
    let strip = app.state.embeds[&2].tabs_rect();
    let close = crate::plugin_tabs::row_shapes(strip, 2, 1, crate::prefs::check(&crate::prefs::PLUGIN_TABS_STACKED)).into_iter().find(|t| t.index == 1).and_then(|t| t.close()).unwrap();
    app.mouse_move(close.cx(), close.cy());
    assert_eq!(app.top_hit(close.cx(), close.cy()).map(|h| h.action.clone()), Some(Action::PluginTabClose(2)));
    assert!(app.embed_down(os_plugkit::input::Button::Left));
    assert!(app.embed_up());
    assert!(app.state.plugin_tabs.group_of(1).is_none() && !app.state.plugin_tabs.hidden(1), "the other shows, alone");
    app.plugin_tick();
    assert!(app.state.z.contains(&Win::Hosted(1)), "in the stack");
    assert!(!app.state.embeds.contains_key(&2) && app.state.embeds.contains_key(&1));
    frame(&mut app, &mut c, None);
    // A window's own close closes all its tabs; with one left, that one.
    let e = app.state.embeds[&1].clone();
    let r = e.rect();
    let button = (0..r.w as i32).map(|d| (r.right() - d as f32, r.y + 15.0)).find(|&(x, y)| e.button_at(x, y) == Some(true)).expect("the close button");
    app.mouse_move(button.0, button.1);
    assert!(app.embed_down(os_plugkit::input::Button::Left));
    assert!(app.embed_up());
    app.plugin_tick();
    app.plugin_tick();
    assert!(app.state.embeds.is_empty(), "all of them closed");
}

/// Zest, compiled in, draws its editor in the app's window as the stock plugins do.
#[cfg(lemon_cards)]
#[test]
fn zest_draws_in_the_app() {
    use crate::plugin_window::ChannelInfo;
    use std::time::{Duration, Instant};
    for name in ["Zest"] {
        let Ok(mut c) = Canvas::new(1536, 864, 1.0) else { return };
        let mut app = App::new(None);
        // as Lemon cards (the app's own theme, not the process-wide one)
        app.state.bar.tiling = true;
        app.theme = design_theme(Design::Lemon, false);
        app.state.session.plugins.available = crate::plugins::builtin();
        app.state.session.plugins.scale = 1.0;
        frame(&mut app, &mut c, None);
        let index = app.state.session.plugins.index_of(name).unwrap_or_else(|| panic!("built-in {name}"));
        app.state.session.plugins.create_with(0, index, None).unwrap();
        let info = ChannelInfo { icon: None, name: name.into(), target: "Insert 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None };
        app.state.editor_host.open_window(&mut app.state.session.plugins, 0, info).unwrap();
        let t0 = Instant::now();
        while !app.state.embeds.get(&0).is_some_and(|e| e.view().frame.is_some()) && t0.elapsed() < Duration::from_secs(5) {
            app.plugin_tick();
            std::thread::sleep(Duration::from_millis(10));
        }
        app.plugin_tick();
        let e = app.state.embeds.get(&0).cloned().unwrap_or_else(|| panic!("{name} drawn in the app"));
        assert!(e.claimed(), "{name}: its editor took the app's surface");
        let f = e.view().frame.unwrap_or_else(|| panic!("{name}: a frame"));
        app.take_damage();
        frame(&mut app, &mut c, None);
        if let Some(dir) = std::env::var_os("EMBED_DIR") {
            c.pixmap.save_png(std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
        }
        let ed = e.editor_rect().unwrap();
        let (x, y) = (20u32.min(f.w - 1), 20u32.min(f.h - 1));
        let p = c.pixmap.pixel(ed.x as u32 + x, ed.y as u32 + y).unwrap();
        let want = f.px[(y * f.w + x) as usize];
        assert_eq!((p.red(), p.green(), p.blue()), ((want >> 16) as u8, (want >> 8) as u8, want as u8), "{name}: its pixels on the canvas");
        app.mouse_move(ed.cx(), ed.cy());
        app.mouse_down();
        app.mouse_up();
        app.state.editor_host.close_window(&mut app.state.session.plugins, 0);
        app.plugin_tick();
    }
}

/// The tiling layout's split gaps drag only where no floating window lies over them: over a floating window, its
/// own clicks and cursor come first.
#[test]
fn floating_windows_cover_split_gaps() {
    let Ok(mut c) = Canvas::new(1600, 900, 1.0) else { return };
    let mut app = App::new(None);
    app.set_tiling_layout(true);
    app.state.raise(Win::Playlist);
    app.state.raise(Win::PianoRoll);
    frame(&mut app, &mut c, None);
    let gap = crate::tiles::plan(&app.state).dividers[0].gap;
    let along = gap.w >= gap.h;
    let at = |k: f32| if along { (gap.x + k * gap.w, gap.cy()) } else { (gap.cx(), gap.y + k * gap.h) };
    let split = |app: &App, (x, y): (f32, f32)| matches!(app.top_hit(x, y).map(|h| &h.action), Some(Action::TileSplit(_)));
    assert!(split(&app, at(0.5)), "the gap drags the split");
    // The instrument rack over the gap's middle.
    app.state.raise(Win::ChannelRack);
    let r = views::rack::rect(&app.state);
    let (mx, my) = at(0.5);
    app.state.rack_pos = (app.state.rack_pos.0 + mx - r.cx(), app.state.rack_pos.1 + my - r.cy());
    app.invalidate();
    frame(&mut app, &mut c, None);
    let r = views::rack::rect(&app.state);
    assert!(r.contains(mx, my));
    assert!(!split(&app, (mx, my)), "the rack's, not the gap's");
    let free = (0..=20).map(|k| at(k as f32 / 20.0)).find(|&(x, y)| !r.outset(8.0).contains(x, y)).expect("a part of the gap the rack leaves");
    assert!(split(&app, free), "the gap still drags where the rack is not");
}

/// A plugin window drawn in the app's over the playlist stays put while the playlist scrolls under it: its pixels
/// do not move with the playlist's, and the result equals a full repaint (no jitter, no old copies left behind).
#[cfg(lemon_cards)]
#[test]
fn playlist_scroll_leaves_in_app_plugin_windows_in_place() {
    use crate::playlist as pl;
    use crate::plugin_window::ChannelInfo;
    use std::time::{Duration, Instant};
    let Ok(mut a) = Canvas::new(1920, 1080, 1.0) else { return };
    let mut b = Canvas::new(1920, 1080, 1.0).unwrap();
    let mut app = App::new(None);
    app.state.session.project = crate::locked::Locked::new(test_project());
    let bar = 4 * u32::from(app.state.session.project.ppq);
    for t in 0..14u16 {
        pl::place(&mut app.state, t, u32::from(t) * bar);
    }
    app.state.z = vec![Win::Playlist];
    frame(&mut app, &mut a, None);
    app.state.session.plugins.available = crate::plugins::builtin();
    app.state.session.plugins.scale = 1.0;
    let index = app.state.session.plugins.index_of(crate::citrus::CITRUS).expect("built-in Citrus");
    app.state.session.plugins.create_with(0, index, None).unwrap();
    let info = ChannelInfo { icon: None, name: "Citrus".into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None };
    app.state.editor_host.open_window(&mut app.state.session.plugins, 0, info).unwrap();
    let t0 = Instant::now();
    while !app.state.embeds.get(&0).is_some_and(|e| e.view().frame.is_some()) && t0.elapsed() < Duration::from_secs(3) {
        app.plugin_tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    let g = app.pl_grid.expect("the playlist's grid");
    app.state.embeds[&0].set_pos((g.x + 420.0, g.y + 60.0));
    app.state.raise(Win::Hosted(0));
    app.plugin_tick();
    app.invalidate();
    let d = app.take_damage();
    frame_damage(&mut app, &mut a, d);
    crate::prefs::set_slider(&crate::prefs::SMOOTHING, 0);
    let over = (g.x + 150.0, g.y + 300.0);
    app.mouse_move(over.0, over.1);
    assert_eq!(app.top_hit(over.0, over.1).map(|h| h.action.clone()), Some(Action::PlaylistArea), "the mouse is over the playlist");
    for (what, dy) in [("down", -1.0), ("down again", -1.0), ("up", 1.0)] {
        app.wheel(0.0, dy, false, false);
        let d = app.take_damage();
        frame_damage(&mut app, &mut a, d);
        frame(&mut app, &mut b, None);
        assert_same(&a, &b, &format!("scrolled {what} under a plugin window"));
    }
}

/// A plugin compiled into Lemon Studio draws its editor in the app's own window (Linux): no window of its own and
/// no display needed. Its frame joins the window stack, the editor's pixels show on the canvas, a click in the
/// editor goes to it, and the frame's close button closes it.
#[cfg(lemon_cards)]
#[test]
fn built_in_editors_draw_in_the_app() {
    use crate::plugin_window::ChannelInfo;
    use std::time::{Duration, Instant};
    let Ok(mut c) = Canvas::new(1536, 864, 1.0) else { return };
    let mut app = App::new(None);
    app.state.session.plugins.available = crate::plugins::builtin();
    app.state.session.plugins.scale = 1.0;
    frame(&mut app, &mut c, None);
    let index = app.state.session.plugins.index_of(crate::citrus::CITRUS).expect("built-in Citrus");
    app.state.session.plugins.create_with(0, index, None).unwrap();
    let info = ChannelInfo { icon: None, name: "Citrus".into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None };
    app.state.editor_host.open_window(&mut app.state.session.plugins, 0, info).unwrap();
    let settle = |app: &mut App, f: &dyn Fn(&App) -> bool| {
        let t0 = Instant::now();
        while !f(app) && t0.elapsed() < Duration::from_secs(3) {
            app.plugin_tick();
            std::thread::sleep(Duration::from_millis(10));
        }
        app.plugin_tick();
    };
    settle(&mut app, &|a| a.state.embeds.get(&0).is_some_and(|e| e.claimed() && e.view().frame.is_some()));
    let e = app.state.embeds.get(&0).cloned().expect("drawn in the app");
    assert!(e.claimed(), "the editor took the app's surface instead of opening a window");
    assert!(app.state.z.contains(&Win::Hosted(0)), "it joins the window stack");
    app.take_damage();
    frame(&mut app, &mut c, None);
    let ed = e.editor_rect().expect("the editor shows");
    let f = e.view().frame.unwrap();
    let (px, py) = ((ed.x + 30.0) as u32, (ed.y + 30.0) as u32);
    let p = c.pixmap.pixel(px, py).unwrap();
    let want = f.px[(30 * f.w + 30) as usize];
    assert_eq!((p.red(), p.green(), p.blue()), ((want >> 16) as u8, (want >> 8) as u8, want as u8), "the editor's pixels on the canvas");
    // a click in the editor is the editor's (nothing else under it moves)
    app.mouse_move(ed.cx(), ed.cy());
    assert_eq!(app.top_hit(ed.cx(), ed.cy()).map(|h| h.action.clone()), Some(Action::Embed(0)));
    app.mouse_down();
    app.mouse_up();
    let r = e.rect();
    // dragged by its title bar above the workspace, it stops with its title bar at the workspace's top
    let (tx, ty) = (r.x + 120.0, r.y + 10.0);
    app.mouse_move(tx, ty);
    app.mouse_down();
    app.mouse_move(tx, 0.0);
    app.mouse_move(tx - 5000.0, -400.0);
    let kept = e.rect();
    app.mouse_up();
    let work = app.state.work_rect();
    assert_eq!(kept.y, work.y, "its top stays in the workspace");
    assert!(kept.right() >= work.x + crate::win_clamp::KEEP_W, "some of it stays in view: {kept:?}");
    app.plugin_tick();
    app.take_damage();
    frame(&mut app, &mut c, None);
    let (cx, cy) = (kept.right() - 23.0 + 8.0, kept.y + 7.0 + 8.0);
    app.mouse_move(cx, cy);
    app.mouse_down();
    app.mouse_up();
    settle(&mut app, &|a| a.state.embeds.is_empty());
    assert!(app.state.embeds.is_empty() && app.state.editor_host.editors.get(&0).is_none_or(|e| e.window.is_none()), "closed by its button");
    // the window manager's close shortcut closes it in front, not the app
    let info = ChannelInfo { icon: None, name: "Citrus".into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None };
    app.state.editor_host.open_window(&mut app.state.session.plugins, 0, info).unwrap();
    settle(&mut app, &|a| a.state.embeds.contains_key(&0));
    app.state.raise(Win::Hosted(0));
    app.close_requested();
    settle(&mut app, &|a| a.state.embeds.is_empty());
    assert!(app.state.editor_host.editors.get(&0).is_none_or(|e| e.window.is_none()) && !app.want_exit, "the shortcut closed the plugin window only");
}

/// The tiling layout's Ctrl+Shift+F closes the window under the mouse, as its close button does: a split window,
/// then a floating one over the split (Ctrl+F stays the browser's search).
#[test]
fn ctrl_shift_f_closes_the_window_under_the_mouse() {
    use crate::keys::Key;
    let Ok(mut c) = Canvas::new(1536, 864, 1.0) else { return };
    let mut app = App::new(None);
    app.state.bar.tiling = true;
    for w in [Win::Playlist, Win::PianoRoll, Win::ChannelRack] {
        app.state.raise(w);
    }
    frame(&mut app, &mut c, None);
    let over = |app: &App, w: Win| app.hits.iter().rev().find(|h| h.win == Some(w)).map(|h| (h.rect.cx(), h.rect.cy())).unwrap();
    let ctrl_shift_f = |app: &mut App, c: &mut Canvas, at: (f32, f32)| {
        app.mouse_move(at.0, at.1);
        app.shift = true;
        app.key(&Key::Character("f".into()), true, false);
        app.shift = false;
        frame(app, c, None);
    };
    let rack = over(&app, Win::ChannelRack);
    assert_eq!(app.top_hit(rack.0, rack.1).and_then(|h| h.win), Some(Win::ChannelRack));
    ctrl_shift_f(&mut app, &mut c, rack);
    assert!(!app.state.shown(Win::ChannelRack), "the floating window under the mouse closed");
    assert!(app.state.shown(Win::PianoRoll) && app.state.shown(Win::Playlist));
    let piano = over(&app, Win::PianoRoll);
    ctrl_shift_f(&mut app, &mut c, piano);
    assert!(!app.state.shown(Win::PianoRoll), "the split window under the mouse closed");
    assert!(app.state.shown(Win::Playlist), "the other stays");
}

/// The Lemon design draws a built-in editor's window as a card (owner, 2026-10-27): rounded like the channel rack,
/// the editor edge to edge below a title row, no strip and no minimize button. `OS_EMBED_SHOT=<file.png>` saves it.
#[cfg(lemon_cards)]
#[test]
fn built_in_editors_are_lemon_cards() {
    use crate::plugin_window::ChannelInfo;
    use std::time::{Duration, Instant};
    let Ok(mut c) = Canvas::new(1536, 864, 1.0) else { return };
    let mut app = App::new(None);
    app.state.bar.tiling = true;
    // The app's own theme (`EmbedFrame::sync`), not the process-wide one the other tests paint with.
    app.theme = design_theme(Design::Lemon, false);
    app.state.session.plugins.available = crate::plugins::builtin();
    app.state.session.plugins.scale = 1.0;
    frame(&mut app, &mut c, None);
    let index = app.state.session.plugins.index_of(crate::citrus::CITRUS).expect("built-in Citrus");
    app.state.session.plugins.create_with(0, index, None).unwrap();
    let info = ChannelInfo { icon: None, name: "Citrus".into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: Some(Default::default()) };
    app.state.editor_host.open_window(&mut app.state.session.plugins, 0, info).unwrap();
    let t0 = Instant::now();
    while !app.state.embeds.get(&0).is_some_and(|e| e.claimed() && e.view().frame.is_some()) && t0.elapsed() < Duration::from_secs(3) {
        app.plugin_tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    app.plugin_tick();
    let e = app.state.embeds.get(&0).cloned().expect("drawn in the app");
    app.take_damage();
    app.invalidate();
    frame(&mut app, &mut c, None);
    if let Some(out) = std::env::var_os("OS_EMBED_SHOT") {
        c.pixmap.save_png(std::path::Path::new(&out)).unwrap();
    }
    let r = e.rect();
    let ed = e.editor_rect().expect("the editor shows");
    assert_eq!((ed.x, ed.y, ed.right(), ed.bottom()), (r.x + 1.0, r.y + crate::plugin_window::CARD_TITLE_H, r.right() - 1.0, r.bottom() - 1.0), "edge to edge below the title row");
    // the corners are rounded: what is below shows at the card's very corner
    let px = |x: f32, y: f32| {
        let p = c.pixmap.pixel(x as u32, y as u32).unwrap();
        (p.red(), p.green(), p.blue())
    };
    let t = &app.theme.colors;
    let border = (t.window_border.r, t.window_border.g, t.window_border.b);
    assert_ne!(px(r.x, r.y), border, "top-left corner rounded");
    assert_ne!(px(r.right() - 1.0, r.bottom() - 1.0), border, "bottom-right corner rounded");
    let f = e.view().frame.unwrap();
    assert_ne!(px(r.x + 1.0, r.bottom() - 2.0), { let p = f.px[((f.h - 1) * f.w) as usize]; ((p >> 16) as u8, (p >> 8) as u8, p as u8) }, "the editor's bottom corner is rounded too");
    // the close button closes it
    app.mouse_move(r.right() - 17.0, r.y + 15.0);
    app.mouse_down();
    app.mouse_up();
    let t0 = Instant::now();
    while !app.state.embeds.is_empty() && t0.elapsed() < Duration::from_secs(3) {
        app.plugin_tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(app.state.embeds.is_empty(), "closed by its button");
}

/// Buttons that act on their release bring their window forward then too, so nothing shows half done while the
/// button is held: the channel's name button over its open plugin window raises the rack and hides the window at
/// the release, and the plugin window's close button with the rack in front never brings the window forward.
#[cfg(lemon_cards)]
#[test]
fn release_buttons_raise_their_window_at_the_release() {
    use crate::plugin_window::ChannelInfo;
    use std::time::{Duration, Instant};
    let Ok(mut c) = Canvas::new(1536, 864, 1.0) else { return };
    let mut app = App::new(None);
    app.state.session.plugins.available = crate::plugins::builtin();
    app.state.session.plugins.scale = 1.0;
    let index = app.state.session.plugins.index_of(crate::citrus::CITRUS).expect("built-in Citrus");
    app.state.session.plugins.create_with(0, index, None).unwrap();
    if !app.state.session.hosted_rows.contains(&0) {
        app.state.session.hosted_rows.push(0);
    }
    let settle = |app: &mut App, f: &dyn Fn(&App) -> bool| {
        let t0 = Instant::now();
        while !f(app) && t0.elapsed() < Duration::from_secs(3) {
            app.plugin_tick();
            std::thread::sleep(Duration::from_millis(10));
        }
        app.plugin_tick();
    };
    let open = |app: &mut App| {
        let info = ChannelInfo { icon: None, name: "Citrus".into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None };
        app.state.editor_host.open_window(&mut app.state.session.plugins, 0, info).unwrap();
        settle(app, &|a| a.state.embeds.contains_key(&0));
        app.state.raise(Win::Hosted(0));
    };
    app.state.raise(Win::ChannelRack);
    open(&mut app);
    // the plugin window off the rack's name buttons
    let name = views::rack::name_rect(&app.state, 0).expect("row 0's name button");
    let e = app.state.embeds[&0].clone();
    e.set_pos((name.right() + 400.0, name.y));
    app.plugin_tick();
    app.invalidate();
    frame(&mut app, &mut c, None);
    app.mouse_move(name.cx(), name.cy());
    app.mouse_down();
    assert_eq!(app.state.z.last(), Some(&Win::Hosted(0)), "held, the button changes nothing yet");
    app.mouse_up();
    assert!(app.state.editor_host.editors.get(&0).is_none_or(|e| e.window.is_none()), "the release hides the plugin window");
    assert_eq!(app.state.z.last(), Some(&Win::ChannelRack), "and brings the rack forward with it");
    // the rack in front: the plugin window's close button closes it without bringing it forward
    open(&mut app);
    app.state.raise(Win::ChannelRack);
    let e = app.state.embeds[&0].clone();
    e.set_pos((name.right() + 400.0, name.y));
    app.plugin_tick();
    app.invalidate();
    frame(&mut app, &mut c, None);
    let r = e.rect();
    let (cx, cy) = (r.right() - 15.0, r.y + 15.0);
    app.mouse_move(cx, cy);
    assert_eq!(app.top_hit(cx, cy).map(|h| h.action.clone()), Some(Action::Embed(0)), "the close button shows");
    app.mouse_down();
    assert_eq!(app.state.z.last(), Some(&Win::ChannelRack), "the close button's press leaves the rack in front");
    app.mouse_up();
    settle(&mut app, &|a| a.state.embeds.is_empty());
    assert!(app.state.editor_host.editors.get(&0).is_none_or(|e| e.window.is_none()), "closed");
    assert_eq!(app.state.z.last(), Some(&Win::ChannelRack));
}

/// Closed and opened again, a plugin window drawn in the app's comes back where it was, and the channel window
/// where it was last moved (before it is moved, next to the rack; spec: ui/channel-rack.md "Left click").
#[cfg(lemon_cards)]
#[test]
fn windows_reopen_where_they_were() {
    use crate::plugin_window::ChannelInfo;
    use std::time::{Duration, Instant};
    let Ok(mut c) = Canvas::new(1536, 864, 1.0) else { return };
    let mut app = App::new(None);
    app.state.session.plugins.available = crate::plugins::builtin();
    app.state.session.plugins.scale = 1.0;
    frame(&mut app, &mut c, None);
    let index = app.state.session.plugins.index_of(crate::citrus::CITRUS).expect("built-in Citrus");
    app.state.session.plugins.create_with(0, index, None).unwrap();
    let open = |app: &mut App| {
        let info = ChannelInfo { icon: None, name: "Citrus".into(), target: "Bus 1".into(), enabled: true, volume: 0.78, pan: 0.0, pitch_cents: 0, range: 2, track: 1, max_track: 125, misc: None };
        app.state.editor_host.open_window(&mut app.state.session.plugins, 0, info).unwrap();
        let t0 = Instant::now();
        while !app.state.embeds.contains_key(&0) && t0.elapsed() < Duration::from_secs(3) {
            app.plugin_tick();
            std::thread::sleep(Duration::from_millis(10));
        }
        app.plugin_tick();
        app.state.embeds[&0].clone()
    };
    let e = open(&mut app);
    e.set_pos((300.0, 220.0));
    app.plugin_tick();
    app.state.editor_host.close_window(&mut app.state.session.plugins, 0);
    app.plugin_tick();
    let e = open(&mut app);
    assert_eq!((e.rect().x, e.rect().y), (300.0, 220.0), "the plugin window reopens where it was");
    // the channel window: next to the rack until moved, then where it was moved
    app.add_clip_channel(ChannelKind::Automation);
    let row = app.state.session.project.channels.len() - 1;
    assert!(app.state.shown(Win::Plugin), "an automation clip's channel window opens");
    let beside = app.state.plugin_pos;
    app.state.hide(Win::Plugin);
    app.state.plugin_moved = true;
    app.state.plugin_pos = (beside.0 + 120.0, beside.1 + 40.0);
    app.open_channel_window(row);
    assert_eq!(app.state.plugin_pos, (beside.0 + 120.0, beside.1 + 40.0), "the channel window reopens where it was moved");
}

/// The status bar's load and memory readings take fixed slots: going from one digit to two moves nothing.
#[test]
fn status_bar_stays_put_as_the_load_changes() {
    let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
    let mut app = App::new(None);
    app.set_tiling_layout(true);
    let place = |app: &mut App, c: &mut Canvas, load: u32| {
        app.state.cpu.now.load = load;
        app.invalidate();
        frame(app, c, None);
        // the touch controller's button in the status bar, left of the readings
        let bar = views::status_rect(&app.state);
        app.hits.iter().find(|h| h.action == Action::ToggleWin(Win::Touch) && bar.contains(h.rect.cx(), h.rect.cy())).expect("in the status bar").rect.x
    };
    let one = place(&mut app, &mut c, 5);
    assert_eq!(place(&mut app, &mut c, 55), one, "two digits");
    assert_eq!(place(&mut app, &mut c, 100), one, "three");
}

/// The tiling layout: the close shortcut closes the floating windows, the one in front first, whichever window has
/// the keyboard (a split window clicked last, or none), before the app.
#[test]
fn close_shortcut_closes_the_rack_in_lemon() {
    let mut app = App::new(None);
    app.state.bar.tiling = true;
    app.state.raise(Win::Mixer);
    app.state.raise(Win::ChannelRack);
    app.close_requested();
    assert!(!app.state.shown(Win::ChannelRack) && !app.want_exit, "the rack closed, not the app");
    app.close_requested();
    assert!(app.want_exit, "the mixer in front: the app");
    // The keyboard in a split window, or in none: the floating windows still close first, front one first.
    let mut app = App::new(None);
    app.state.bar.tiling = true;
    app.state.raise(Win::ChannelRack);
    app.state.raise(Win::Tapper);
    app.state.raise(Win::Mixer);
    app.close_requested();
    assert!(!app.state.shown(Win::Tapper) && app.state.shown(Win::ChannelRack) && !app.want_exit, "the tapper first");
    app.state.no_focus = true;
    app.close_requested();
    assert!(!app.state.shown(Win::ChannelRack) && !app.want_exit, "then the rack");
    app.close_requested();
    assert!(app.want_exit, "then the app");
}

/// In a narrow, tall note grid (a split view) notes above it on its left and on its right lie opposite ways: both get
/// an arrow, as they do with the grid maximized.
#[test]
fn piano_arrows_tell_left_from_right_in_a_narrow_grid() {
    let mut app = App::new(None);
    let ch = app.state.session.project.channels[app.state.selected_channel].index;
    let ppq = u32::from(app.state.session.project.ppq.max(4));
    crate::piano::center_keys(&mut app.state, 900.0);
    app.scroll_set(Scroll::PianoX, 2000.0);
    for (w, h) in [(300.0, 900.0), (1600.0, 700.0)] {
        let grid = Rect::new(100.0, 100.0, w, h);
        let g = crate::piano::Geom::new(&app.state, grid);
        let right = g.tick_at(grid.right() + 200.0) as u32;
        let notes = vec![Note { key: 127, length: ppq, ..Note::step(ch, 0) }, Note { key: 127, length: ppq, ..Note::step(ch, right) }];
        for n in &notes {
            let r = g.note_rect(n).expect("a row");
            assert!(r.bottom() <= grid.y && (r.right() <= grid.x || r.x >= grid.right()), "{w}x{h}: {r:?} above and to a side of {grid:?}");
        }
        assert_eq!(crate::piano::off_view_notes(&g, &notes, Some(ch)).len(), 2, "{w}x{h}: an arrow each way");
    }
}

/// Notes of one key at once show stacked: a layer above the one lying on another, over the time they share only;
/// the notes keep their places.
#[test]
fn notes_on_one_key_at_once_look_stacked() {
    let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
    let mut app = App::new(None);
    app.state.z = vec![Win::PianoRoll];
    let ch = app.state.session.project.channels[app.state.selected_channel].index;
    let grid = views::piano::grid_rect(&app.state);
    crate::piano::center_keys(&mut app.state, grid.h);
    let g = crate::piano::Geom::new(&app.state, grid);
    let at = |x: f32| g.tick_at(x) as u32;
    let (a, b, d) = (at(grid.x + 100.0), at(grid.x + 400.0), at(grid.x + 250.0) - at(grid.x + 100.0));
    let pattern = app.state.pattern();
    let pat = app.state.session.project.get_mut().patterns.iter_mut().find(|p| p.number == pattern).unwrap();
    // Two copies of one note on C5; on D5 a note, and one starting halfway along it.
    pat.notes = vec![
        Note { key: 60, length: d, ..Note::step(ch, a) },
        Note { key: 60, length: d, ..Note::step(ch, a) },
        Note { key: 62, length: d, ..Note::step(ch, b) },
        Note { key: 62, length: d, ..Note::step(ch, b + d / 2) },
    ];
    let notes = pat.notes.clone();
    frame(&mut app, &mut c, None);
    let rim = app.theme.colors.note_rim;
    let is_rim = |c: &Canvas, x: f32, y: f32| {
        let p = c.pixmap.pixel(x as u32, y as u32).unwrap();
        (p.red(), p.green(), p.blue()) == (rim.r, rim.g, rim.b)
    };
    let copy = g.note_rect(&notes[1]).unwrap();
    assert!(is_rim(&c, copy.cx(), copy.y - 2.0), "a layer above the copy");
    let (first, second) = (g.note_rect(&notes[2]).unwrap(), g.note_rect(&notes[3]).unwrap());
    assert!(is_rim(&c, (second.x + first.right()) / 2.0, second.y - 2.0), "over the time they share");
    assert!(!is_rim(&c, (first.right() + second.right()) / 2.0, second.y - 2.0), "not past it");
    assert!(!is_rim(&c, (first.x + second.x) / 2.0, first.y - 2.0), "none where it lies alone");
}

/// The channel's notes are all out of view: an arrow at the grid's edge points at the nearest and names it; a click
/// brings it into view, and the arrow goes.
#[test]
fn piano_arrow_finds_notes_out_of_view() {
    let Ok(mut c) = Canvas::new(1920, 1080, 1.0) else { return };
    let mut app = App::new(None);
    app.state.z = vec![Win::PianoRoll];
    let ch = app.state.session.project.channels[app.state.selected_channel].index;
    let ppq = u32::from(app.state.session.project.ppq.max(4));
    // C5 centred, then notes far above and below the rows shown around it.
    let pattern = app.state.pattern();
    app.state.session.project.get_mut().patterns.iter_mut().find(|p| p.number == pattern).unwrap().notes.clear();
    let h = views::piano::grid_rect(&app.state).h;
    crate::piano::center_keys(&mut app.state, h);
    // Both under the grid's middle, so one is straight above and the other straight below.
    let mid = crate::piano::Geom::new(&app.state, views::piano::grid_rect(&app.state)).tick_at(views::piano::grid_rect(&app.state).cx()) as u32;
    let pat = app.state.session.project.get_mut().patterns.iter_mut().find(|p| p.number == pattern).unwrap();
    pat.notes = vec![Note { key: 125, length: ppq, ..Note::step(ch, mid) }, Note { key: 2, length: ppq, ..Note::step(ch, mid) }];
    frame(&mut app, &mut c, None);
    let g = crate::piano::Geom::new(&app.state, views::piano::grid_rect(&app.state));
    let (i, p) = crate::piano::off_view_note(&g, &app.state.session.project.patterns.iter().find(|p| p.number == app.state.pattern()).unwrap().notes, Some(ch)).expect("none in view");
    // One arrow each way, the nearest note's the larger.
    let notes_now = app.state.session.project.patterns.iter().find(|p| p.number == app.state.pattern()).unwrap().notes.clone();
    let both = crate::piano::off_view_notes(&g, &notes_now, Some(ch));
    assert_eq!(both.len(), 2, "one above, one below");
    let other = hit_rect(&app, &Action::PianoSeek(both[1].0));
    assert!(other.w < hit_rect(&app, &Action::PianoSeek(i)).w, "the farther note's arrow is smaller");
    let r = hit_rect(&app, &Action::PianoSeek(i));
    if let Some(out) = std::env::var_os("OS_SEEK_SHOT") {
        c.pixmap.save_png(std::path::Path::new(&out)).unwrap();
    }
    let grid = views::piano::grid_rect(&app.state);
    assert!(grid.contains(r.cx(), r.cy()), "the arrow is on the grid");
    assert_eq!(p.1 < grid.cy(), r.cy() < grid.cy(), "on the side of the nearest note");
    click(&mut app, &mut c, r);
    app.ease_to_end();
    app.invalidate();
    frame(&mut app, &mut c, None);
    assert!(app.hits.iter().all(|h| !matches!(h.action, Action::PianoSeek(_))), "a note is in view: no arrow");
    let g = crate::piano::Geom::new(&app.state, grid);
    let n = app.state.session.project.patterns.iter().find(|p| p.number == app.state.pattern()).unwrap().notes[i];
    let nr = g.note_rect(&n).unwrap();
    // Centred as far as the roll scrolls (the lowest and highest keys stop at its ends).
    assert!(nr.intersect(&grid).h >= nr.h, "the note is in view: {nr:?} in {grid:?}");
}

/// The tiling layout: the workspace is a card with a gap all round, above the status bar; the browser ends at the
/// status bar too. The classic layout's workspace runs to the window's edges.
#[test]
fn layouts_keep_their_own_state() {
    use crate::toolbar::{Layout, PRESETS, TILING_PRESET};
    let mut app = App::new(None);
    // Classic: a custom toolbar (one row of the classic), the mixer floating, the playlist maximized.
    let mut mine = Layout::classic();
    mine.set_rows(1);
    app.set_toolbar_layout(mine.clone());
    app.state.raise(Win::Mixer);
    app.state.wins[1].maxed = false;
    let mixer_at = app.state.wins[1].rect;
    app.set_tiling_layout(true);
    assert_eq!(app.state.bar.layout, PRESETS[TILING_PRESET].layout());
    // Lemon: the mixer hidden, the piano roll maximized.
    app.act(Action::ToggleWin(Win::Mixer));
    app.act(Action::ShowMax(Win::PianoRoll));
    app.set_tiling_layout(false);
    assert_eq!(app.state.bar.layout, mine, "the classic toolbar back");
    assert!(app.state.shown(Win::Mixer) && !app.state.shown(Win::PianoRoll) && app.state.wins[1].rect == mixer_at);
    assert!(app.state.is_max(Win::Playlist) && !app.state.is_max(Win::Mixer));
    app.set_tiling_layout(true);
    assert!(!app.state.shown(Win::Mixer), "{:?}", app.state.z);
    assert!(app.state.is_max(Win::PianoRoll), "{:?}", app.state.z);
    assert_eq!((Layout::load_for(false), Layout::load_for(true)), (mine, PRESETS[TILING_PRESET].layout()));
}

#[test]
fn lemon_layout_geometry() {
    let mut app = App::new(None);
    app.state.screen = (1536.0, 864.0);
    let classic = app.state.work_rect();
    assert_eq!(classic.bottom(), 864.0);
    app.state.bar.tiling = true;
    app.state.bar.layout = crate::toolbar::PRESETS[crate::toolbar::TILING_PRESET].layout();
    let w = app.state.work_rect();
    let g = views::GUTTER;
    assert_eq!(w.bottom(), 864.0 - views::STATUS_H - g);
    assert_eq!(w.right(), 1536.0 - g);
    assert_eq!(w.y, views::work_y(&app.state) + g);
    assert_eq!(views::work_y(&app.state), crate::toolbar::ROW_H + 1.0, "one row");
    assert_eq!(views::status_rect(&app.state).y, 864.0 - views::STATUS_H);
    // the browser is a card too, the splitter the gap between the two
    app.state.show_browser = true;
    let (b, w) = (views::browser_rect(&app.state), app.state.work_rect());
    assert_eq!((b.x, b.y, b.bottom()), (g, w.y, w.bottom()));
    assert_eq!(w.x, b.right());
    // docked right: the same card on the other side, the workspace left of it
    app.state.browser_right = true;
    let (b, w) = (views::browser_rect(&app.state), app.state.work_rect());
    assert_eq!((w.x, w.right(), b.right()), (g, b.x, 1536.0 - g));
    assert_eq!(views::browser_panel(&app.state, b).x, b.x + g);
    app.state.browser_right = false;
    app.state.show_browser = false;
    assert_eq!(app.state.work_rect().x, g);
}
