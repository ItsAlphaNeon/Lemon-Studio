// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The interface's side of `crate::presets` (step 9: the session's side stays there).

#![allow(unused_imports)]
use std::path::{Path, PathBuf};
use crate::app::App;
use crate::playlist::{Cmd, Mark, PItem, Popup};
#[allow(unused_imports)]
use crate::presets::*;

/// The preset header of the instance under `key`.
fn header(app: &App, key: usize) -> Option<Header> {
    let d = app.state.session.plugins.descriptor(key)?;
    Some(Header { plugin: d.name.clone(), id: d.uid(), format: d.format.as_str().into(), body: String::new() })
}

/// The current preset of the instance under `key` when it is a file of the user's presets (one that can be renamed and
/// deleted; factory presets and files dropped from elsewhere cannot).
fn own_current(app: &App, key: usize) -> Option<PathBuf> {
    let p = app.state.session.plugins.presets.get(&key)?;
    (!is_factory(p) && p.is_file() && root().is_some_and(|r| p.starts_with(r))).then(|| p.clone())
}

/// Rows of the presets in `list`: sub-folder presets in one submenu per folder when `nested`, else "Folder/name" rows.
fn rows(app: &App, key: usize, list: &[Preset], nested: bool) -> Vec<PItem> {
    let cur = app.state.session.plugins.presets.get(&key);
    let row = |p: &Preset, label: String| PItem::new(label, Cmd::Preset(PCmd::Load(key, p.path.clone()))).mark(Mark::Check(cur == Some(&p.path)));
    let mut v: Vec<PItem> = Vec::new();
    let mut folders: Vec<(String, Vec<PItem>)> = Vec::new();
    for p in list {
        match p.name.split_once('/').filter(|_| nested) {
            Some((folder, stem)) => match folders.iter_mut().find(|(f, _)| f == folder) {
                Some((_, items)) => items.push(row(p, stem.to_string())),
                None => folders.push((folder.to_string(), vec![row(p, stem.to_string())])),
            },
            None => v.push(row(p, p.name.clone())),
        }
    }
    for (folder, children) in folders {
        v.push(PItem { children, ..PItem::new(folder, Cmd::Nothing) });
    }
    v
}

/// The preset list rows for the instance under `key`: the user's presets, then the factory ones (under "User" and
/// "Factory" heads while there are both). `nested`: presets in sub-folders go into one submenu per folder (the
/// title-bar box); otherwise every preset is a row, sub-folder ones as "Folder/name" (the window menu's Presets
/// submenu, which cannot open a further submenu). The plugin window's popups are Windows-only so far.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn items(app: &App, key: usize, nested: bool) -> Vec<PItem> {
    let Some(h) = header(app, key) else { return Vec::new() };
    let (mine, fac) = (user(&h.plugin), factory(&app.state.session.settings, &h.plugin));
    let both = !mine.is_empty() && !fac.is_empty();
    let mut v: Vec<PItem> = Vec::new();
    if both {
        v.push(PItem::head("User"));
    }
    v.extend(rows(app, key, &mine, nested));
    if both {
        v.push(PItem::head("Factory"));
    }
    v.extend(rows(app, key, &fac, nested));
    if v.is_empty() {
        v.push(PItem::new("(no presets)", Cmd::Nothing).off());
    }
    // Long lists continue in further columns (the popup wraps at the screen's height).
    v
}

/// Fills the Presets section of the window menu and of the mixer's slot menu (spec: ui/plugin-window.md "Presets"):
/// "Presets" becomes the list (a submenu), "Browse presets" and "Save preset as..." work.
pub fn fill_window_menu(app: &App, key: usize, menu: &mut Popup) {
    let on = |it: PItem, on: bool| if on { it } else { it.off() };
    for it in menu.items.iter_mut() {
        match &*it.label {
            "Presets" if !it.head => *it = PItem { children: items(app, key, false), ..PItem::new("Presets", Cmd::Nothing) },
            "Browse presets" => *it = PItem::new("Browse presets", Cmd::Preset(PCmd::Browse(key))),
            "Save preset as..." => *it = PItem::new("Save preset as...", Cmd::Preset(PCmd::SaveAs(key))),
            // The plugin's star in the plugin manager (spec: ui/browser.md §10).
            "Add to favourites" => *it = PItem::new(it.label.clone(), Cmd::Browser(crate::browser::BCmd::Star(key))),
            "Display preset name in preset selector" => {
                let shown = app.state.session.plugins.hosted.get(&key).map(|h| !h.hide_preset);
                *it = on(PItem::new(it.label.clone(), Cmd::Preset(PCmd::ShowName(key, shown != Some(true)))).mark(Mark::Check(shown != Some(false))), shown.is_some());
            }
            // The spare state (spec: ui/plugin-window.md "Spare state"): Flip is greyed until a state is stored.
            "Copy to B" => *it = on(PItem::new(it.label.clone(), Cmd::Preset(PCmd::StoreSpare(key))), app.state.session.plugins.hosted.contains_key(&key)),
            "Swap A / B" => *it = on(PItem::new(it.label.clone(), Cmd::Preset(PCmd::FlipSpare(key))), app.state.session.plugins.hosted.get(&key).is_some_and(|h| h.spare.is_some())),
            _ => {}
        }
    }
}

/// The title-bar Presets box popup: the presets, then the commands on them.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn popup(app: &App, key: usize, x: f32, y: f32) -> Popup {
    let mut items = items(app, key, true);
    let own = own_current(app, key).is_some();
    let on = |it: PItem, on: bool| if on { it } else { it.off() };
    items.push(PItem::sep());
    items.push(PItem::new("Save preset as...", Cmd::Preset(PCmd::SaveAs(key))));
    items.push(PItem::new("Import preset...", Cmd::Preset(PCmd::Import(key))));
    items.push(on(PItem::new("Rename preset...", Cmd::Preset(PCmd::Rename(key))), own));
    items.push(on(PItem::new("Delete preset...", Cmd::Preset(PCmd::Delete(key))), own));
    items.push(PItem::sep());
    items.push(PItem::new("Browse presets", Cmd::Preset(PCmd::Browse(key))));
    Popup { x, y, items, sub: None, scroll: [0; 2] }
}

/// Runs a preset command; returns the status message.
pub fn run(app: &mut App, c: PCmd) -> Option<String> {
    let msg = match c {
        PCmd::Load(key, path) => load_into(app, key, &path),
        PCmd::Step(key, dir) => {
            let h = header(app, key)?;
            let l = list(&app.state.session.settings, &h.plugin);
            match step(&l, app.state.session.plugins.presets.get(&key).map(PathBuf::as_path), dir) {
                Some(i) => load_into(app, key, &l[i].path),
                None => Some(format!("No presets for {} yet (Save preset as... stores one)", h.plugin)),
            }
        }
        PCmd::SaveAs(key) => save_as(app, key),
        PCmd::Import(key) => {
            header(app, key)?;
            let dlg = crate::filedlg::FileDlg::new(crate::filedlg::Mode::Open, "Import preset", "preset import", crate::dialog::Then::ImportPreset(key))
                .filter("Presets", IMPORT_EXTS)
                .filter("All files", &[]);
            app.file_dialog(dlg);
            None
        }
        PCmd::Rename(key) => {
            let cur = own_current(app, key)?;
            let dlg = crate::filedlg::FileDlg::new(crate::filedlg::Mode::Save, "Rename preset", "preset", crate::dialog::Then::RenamePreset(key, cur.clone()))
                .filter("Lemon Studio preset", &[EXT])
                .start(cur.parent())
                .name(&format!("{}.{EXT}", shown_name(&cur)));
            app.file_dialog(dlg);
            None
        }
        PCmd::Delete(key) => {
            let cur = own_current(app, key)?;
            use crate::dialog::{Answer, Kind, Msg, Then};
            let text = vec![("Delete the preset ".to_string(), false), (shown_name(&cur), true), ("?".to_string(), false)];
            app.show_msg(Msg::new(Kind::Question, "Delete preset", text, &[Answer::Yes, Answer::No], Then::DeletePreset(key, cur)));
            None
        }
        PCmd::Browse(key) => browse(app, key),
        PCmd::StoreSpare(key) => {
            let state = app.state.session.plugins.state(key)?;
            app.state.session.plugins.hosted.get_mut(&key)?.spare = Some(state);
            Some("Copied to B".into())
        }
        PCmd::FlipSpare(key) => flip_spare(&mut app.state.session, key),
        PCmd::ShowName(key, show) => {
            app.state.session.plugins.show_preset_name(key, show);
            None
        }
    };
    app.invalidate();
    msg
}

/// Loads preset `path` (any [`IMPORT_EXTS`] type, or a factory preset) into the instance under `key` at the user's
/// request; returns the status message. One undo step, "Plugin preset", that brings back the state, the preset shown
/// and the channel's or slot's name (`control::state_edit`). Lemon Studio's choice: the original's own behaviour was
/// not looked at.
pub fn load_into(app: &mut App, key: usize, path: &Path) -> Option<String> {
    load_with(&mut app.state.session, key, path, true)
}

/// Save preset as...: the save dialog, in the plugin's preset folder (`save_chosen` writes the file).
fn save_as(app: &mut App, key: usize) -> Option<String> {
    let h = header(app, key)?;
    if app.state.session.plugins.state(key).is_none() {
        return Some(format!("{} has no state to save", h.plugin));
    }
    let dir = dir_for(&h.plugin)?;
    let _ = std::fs::create_dir_all(&dir);
    let start = app.state.session.plugins.presets.get(&key).map(|p| shown_name(p)).unwrap_or_else(|| "New preset".into());
    let dlg = crate::filedlg::FileDlg::new(crate::filedlg::Mode::Save, "Save preset as", "preset", crate::dialog::Then::SavePreset(key))
        .filter("Lemon Studio preset", &[EXT])
        .start(Some(&dir))
        .name(&format!("{start}.{EXT}"));
    app.file_dialog(dlg);
    None
}

/// The preset file chosen in Save preset as...: the plugin's state is written there. Returns the status message.
pub fn save_chosen(app: &mut App, key: usize, target: PathBuf) -> Option<String> {
    let h = header(app, key)?;
    let Some(state) = app.state.session.plugins.state(key) else { return Some(format!("{} has no state to save", h.plugin)) };
    let target = with_ext(target);
    Some(match save(&target, &h, &state) {
        Ok(()) => {
            app.state.session.plugins.set_preset(key, Some(target.clone()));
            format!("Saved preset {}", shown_name(&target))
        }
        Err(e) => format!("Preset not saved: {e}"),
    })
}

/// The file chosen in Import preset...: its presets are copied into the plugin's folder (a bank's into a sub-folder
/// named after it), as Lemon Studio presets, and the first one loads. Returns the status message.
pub fn import_chosen(app: &mut App, key: usize, file: PathBuf) -> Option<String> {
    let h = header(app, key)?;
    let d = app.state.session.plugins.descriptor(key)?;
    let found = match read_any(&file, &d) {
        Ok(v) if !v.is_empty() => v,
        Ok(_) => return Some(format!("{} holds no preset", shown_name(&file))),
        Err(e) => return Some(e),
    };
    let mut dir = dir_for(&h.plugin)?;
    if found.len() > 1 {
        dir = dir.join(folder_name(&shown_name(&file)));
    }
    let mut first = None;
    for (name, body) in &found {
        let target = free_file(&dir, name);
        if let Err(e) = save_body(&target, &h, body) {
            return Some(format!("Preset not imported: {e}"));
        }
        first.get_or_insert(target);
    }
    let loaded = load_into(app, key, first.as_deref()?);
    Some(match found.len() {
        1 => loaded.unwrap_or_default().replacen("Preset:", "Imported preset", 1),
        n => format!("Imported {n} presets from {}", shown_name(&file)),
    })
}

/// The new name chosen in Rename preset... for preset `old`. Returns the status message.
pub fn rename_chosen(app: &mut App, key: usize, old: PathBuf, target: PathBuf) -> Option<String> {
    let target = with_ext(target);
    if target == old {
        return None;
    }
    Some(match std::fs::rename(&old, &target) {
        Ok(()) => {
            if app.state.session.plugins.presets.get(&key) == Some(&old) {
                app.state.session.plugins.set_preset(key, Some(target.clone()));
            }
            format!("Renamed preset {} to {}", shown_name(&old), shown_name(&target))
        }
        Err(e) => format!("Preset not renamed: {e}"),
    })
}

/// Delete preset... answered Yes for preset `path`. Returns the status message.
pub fn delete_chosen(app: &mut App, key: usize, path: PathBuf) -> Option<String> {
    Some(match std::fs::remove_file(&path) {
        Ok(()) => {
            if app.state.session.plugins.presets.get(&key) == Some(&path) {
                app.state.session.plugins.set_preset(key, None);
            }
            format!("Deleted preset {}", shown_name(&path))
        }
        Err(e) => format!("Preset not deleted: {e}"),
    })
}

/// Browse presets: the browser shows, on its All tab, Plugin presets opened at the plugin's folder.
fn browse(app: &mut App, key: usize) -> Option<String> {
    let h = header(app, key)?;
    let s = &mut app.state;
    s.show_browser = true;
    s.browser_tab = 0;
    s.browser_search.clear();
    let root = crate::views::browser::presets_key();
    let k = format!("{root}/{}", folder_name(&h.plugin));
    s.browser_open.insert(root);
    s.browser_open.insert(k.clone());
    crate::browser::select(s, &k);
    crate::browser::reveal(s);
    None
}

/// A preset file dropped at the mouse (from the desktop or the browser): into the plugin window under it, the plugin of
/// the rack row or mixer slot under it, else into a new channel (a generator) or effect (in the slot under the mouse,
/// else the current track's first free one) of the plugin the preset is for. Returns the status message.
pub fn dropped(app: &mut App, path: &Path) -> Option<String> {
    use crate::app::{Action, Win};
    // A channel state (listed with the presets in the browser) adds its channel.
    if crate::channel_states::is_state_file(path) {
        return crate::channel_states::add(app, path);
    }
    #[cfg(any(windows, target_os = "macos"))]
    if let Some(row) = app.plugin_window_under_cursor() {
        return load_into(app, row, path);
    }
    let (x, y) = app.mouse;
    let (win, mix) = match app.top_hit(x, y) {
        Some(h) => (h.win, match h.action {
            Action::Mix(crate::mixer::MHit::SlotName(k) | crate::mixer::MHit::SlotArrow(k)) => Some(k),
            _ => None,
        }),
        None => (None, None),
    };
    let mut slot = None;
    match win {
        Some(Win::ChannelRack) => {
            if let Some(row) = crate::views::rack::row_at(&app.state, y).filter(|&r| app.state.session.plugins.descriptor(r).is_some()) {
                return load_into(app, row, path);
            }
        }
        Some(Win::Mixer) => {
            if let Some(k) = mix {
                let key = crate::plugins::fx_key(app.state.selected_mixer, k);
                if app.state.session.plugins.descriptor(key).is_some() {
                    return load_into(app, key, path);
                }
                slot = Some(k);
            }
        }
        _ => {}
    }
    let name = shown_name(path);
    let Some(uid) = uid_of(path) else { return Some(format!("{name}: drop it on a plugin window, a channel or a mixer slot")) };
    let same = |d: &os_host::Descriptor| crate::plugins::same_uid(&d.uid(), &uid);
    if let Some(i) = app.state.session.plugins.available.iter().position(|a| same(&a.desc)) {
        crate::plugin_menu::add_generator(app, i, None);
        let row = app.state.session.project.channels.len() - 1;
        app.state.session.plugins.preset_waits.push((row, path.to_path_buf(), std::time::Instant::now()));
        return Some(format!("Loading {name} in a new channel"));
    }
    if let Some(i) = app.state.session.plugins.effects.iter().position(|a| same(&a.desc)) {
        let t = app.state.selected_mixer;
        let slot = slot.or_else(|| app.state.session.project.mixer.iter().find(|m| m.index == t).and_then(|m| m.slots.iter().position(Option::is_none)).map(|k| k as u8));
        let Some(k) = slot else { return Some("Every slot of this track is in use".into()) };
        crate::plugin_menu::add_effect(app, i, Some(k));
        app.state.session.plugins.preset_waits.push((crate::plugins::fx_key(t, k), path.to_path_buf(), std::time::Instant::now()));
        return Some(format!("Loading {name} in a new effect"));
    }
    let plugin = std::fs::read(path).ok().and_then(|b| decode(&b).map(|(h, _)| h.plugin)).filter(|p| !p.is_empty());
    Some(format!("{name} is for {}, which is not installed", plugin.unwrap_or_else(|| "a plugin".into())))
}

/// Presets dropped where a new instance was made load once it is hosted (or are given up after [`WAIT`]).
pub fn pending_tick(app: &mut App) {
    if !app.state.session.plugins.preset_waits.is_empty() {
        app.host(|ses| pending_in(ses));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Held by the tests that point `OPENSTUDIO_PRESETS` (process-wide) at their own folder.
    static PRESETS_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("os-presets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A hosted Lemon Sampler on rack row 0, presets in a temp folder (`OPENSTUDIO_PRESETS`, kept while the guard is).
    fn hosted_sampler(name: &str) -> (App, PathBuf, std::sync::MutexGuard<'static, ()>) {
        let guard = PRESETS_ENV.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tmp(name);
        // SAFETY: tests in this process that read OPENSTUDIO_PRESETS all point it at a temp folder.
        unsafe { std::env::set_var("OPENSTUDIO_PRESETS", &dir) };
        let mut app = crate::app::App::new(None);
        let mut pl = crate::plugins::Plugins::default();
        pl.available = crate::plugins::builtin();
        let index = pl.index_of(crate::plugins::LEMON_SAMPLER).expect("built in");
        pl.create(0, index).unwrap();
        app.state.session.plugins = pl;
        (app, dir, guard)
    }

    /// Save, list, step and load through a hosted Lemon Sampler: the state goes through the hosting layer and back.
    #[test]
    fn presets_of_a_hosted_plugin() {
        let (mut app, dir, _env) = hosted_sampler("hosted");
        let h = header(&app, 0).unwrap();
        assert_eq!(h.plugin, "Lemon Sampler");
        assert!(h.id.starts_with("clap:"));
        let state = app.state.session.plugins.state(0).unwrap();
        // Two presets: the default state, and one with a sample path.
        save(&file_for(&dir_for(&h.plugin).unwrap(), "Default"), &h, &state).unwrap();
        let other = crate::plugins::with_sample_path(&state, "C:/x/rim.wav").unwrap();
        save(&file_for(&dir_for(&h.plugin).unwrap(), "Rim"), &h, &other).unwrap();
        let names: Vec<String> = user(&h.plugin).iter().map(|p| p.name.clone()).collect();
        assert_eq!(names, ["Default", "Rim"]);
        assert!(run(&mut app, PCmd::Step(0, -1)).unwrap().contains("Rim") || !factory(&crate::settings_file::store(), &h.plugin).is_empty(), "back from no preset = last");
        run(&mut app, PCmd::Load(0, file_for(&dir_for(&h.plugin).unwrap(), "Rim")));
        assert_eq!(app.state.session.plugins.sampler_path(0).as_deref(), Some("C:/x/rim.wav"));
        assert_eq!(app.state.session.plugins.preset_name(0).as_deref(), Some("Rim"));
        // spec: ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup": "Name channels after their sample or plugin" is
        // on by default.
        assert_eq!(app.state.session.project.channels[0].name.as_deref(), Some("Rim"), "the ◀ ▶ arrows rename the channel");
        run(&mut app, PCmd::Step(0, -1));
        assert_eq!(app.state.session.plugins.sampler_path(0).as_deref(), Some(""), "back to Default");
        let it = items(&app, 0, true);
        assert!(it.iter().any(|i| i.label == "Default" && i.mark == Mark::Check(true)));
        // The window menu's Presets entry lists them; Browse presets works.
        let mut m = crate::playlist::plugin_window_menu(0, false, 0.0, 0.0);
        fill_window_menu(&app, 0, &mut m);
        let p = m.items.iter().find(|i| i.label == "Presets" && !i.head).unwrap();
        assert!(p.children.iter().filter(|i| !i.head).count() >= 2);
        assert!(m.items.iter().any(|i| i.cmd == Cmd::Preset(PCmd::SaveAs(0))));
        assert!(m.items.iter().any(|i| i.cmd == Cmd::Preset(PCmd::Browse(0))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An effect inside a container has its presets as a slot's effect has (Lemon Studio's design: spec
    /// plugins/patcher/README.md §4.2): the window menu and the title-bar box list them and a preset loads into it; no
    /// channel is renamed after it.
    #[test]
    fn presets_of_a_container_member() {
        let (mut app, dir, _env) = hosted_sampler("member");
        let key = crate::plugins::MEMBER_KEY;
        let index = app.state.session.plugins.index_of(crate::plugins::LEMON_SAMPLER).unwrap();
        app.state.session.plugins.create(key, index).unwrap();
        let h = header(&app, key).unwrap();
        let other = crate::plugins::with_sample_path(&app.state.session.plugins.state(key).unwrap(), "C:/x/rim.wav").unwrap();
        let rim = file_for(&dir_for(&h.plugin).unwrap(), "Rim");
        save(&rim, &h, &other).unwrap();
        let names: Vec<Option<String>> = app.state.session.project.channels.iter().map(|c| c.name.clone()).collect();
        assert_eq!(run(&mut app, PCmd::Load(key, rim.clone())).as_deref(), Some("Preset: Rim"));
        assert_eq!(app.state.session.plugins.sampler_path(key).as_deref(), Some("C:/x/rim.wav"));
        assert_eq!(app.state.session.plugins.preset_name(key).as_deref(), Some("Rim"));
        assert_eq!(app.state.session.project.channels.iter().map(|c| c.name.clone()).collect::<Vec<_>>(), names);
        let mut m = crate::playlist::plugin_window_menu(key, false, 0.0, 0.0);
        fill_window_menu(&app, key, &mut m);
        let p = m.items.iter().find(|i| i.label == "Presets" && !i.head).unwrap();
        assert!(p.children.iter().any(|i| i.cmd == Cmd::Preset(PCmd::Load(key, rim.clone())) && i.mark == Mark::Check(true)));
        assert!(m.items.iter().any(|i| i.cmd == Cmd::Preset(PCmd::SaveAs(key))));
        assert!(popup(&app, key, 0.0, 0.0).items.iter().any(|i| i.label == "Rim"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Copy to B keeps a copy of the state; Swap A / B (greyed until then) swaps the state in use
    /// with it, one undo step that puts the state in use back. The plugin window menu and the mixer's slot menu have both.
    #[test]
    fn spare_state_store_and_flip() {
        let (mut app, dir, _env) = hosted_sampler("spare");
        let entry = |app: &App, label: &str| {
            let mut m = crate::playlist::plugin_window_menu(0, false, 0.0, 0.0);
            fill_window_menu(app, 0, &mut m);
            m.items.into_iter().find(|i| i.label == label).unwrap()
        };
        assert_eq!(entry(&app, "Copy to B").cmd, Cmd::Preset(PCmd::StoreSpare(0)));
        assert!(entry(&app, "Copy to B").enabled);
        assert!(!entry(&app, "Swap A / B").enabled, "nothing stored yet");
        let a = crate::plugins::with_sample_path(&app.state.session.plugins.state(0).unwrap(), "C:/x/a.wav").unwrap();
        assert!(app.state.session.plugins.set_state(0, &a));
        assert_eq!(run(&mut app, PCmd::StoreSpare(0)).as_deref(), Some("Copied to B"));
        assert!(entry(&app, "Swap A / B").enabled);
        let b = crate::plugins::with_sample_path(&a, "C:/x/b.wav").unwrap();
        assert!(app.state.session.plugins.set_state(0, &b));
        let steps = app.state.session.history.at();
        assert_eq!(run(&mut app, PCmd::FlipSpare(0)).as_deref(), Some("Swapped A / B"));
        assert_eq!(app.state.session.plugins.sampler_path(0).as_deref(), Some("C:/x/a.wav"), "the stored state is in use");
        assert_eq!(app.state.session.history.newest_done(), Some(FLIP_SPARE));
        assert_eq!(app.state.session.history.at(), steps + 1);
        run(&mut app, PCmd::FlipSpare(0));
        assert_eq!(app.state.session.plugins.sampler_path(0).as_deref(), Some("C:/x/b.wav"), "and back");
        crate::history_ui::undo(&mut app.state);
        app.host_commands();
        assert_eq!(app.state.session.plugins.sampler_path(0).as_deref(), Some("C:/x/a.wav"), "undo puts the state in use back");
        // The mixer's slot menu fills the same entries for the slot's plugin.
        let key = crate::plugins::fx_key(app.state.selected_mixer, 0);
        let m = crate::mixer::slot_menu(&app, 0, 0.0, 0.0);
        assert!(m.items.iter().any(|i| i.cmd == Cmd::Preset(PCmd::StoreSpare(key))));
        assert!(m.items.iter().any(|i| i.cmd == Cmd::Preset(PCmd::FlipSpare(key)) && !i.enabled), "no instance in the slot");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Display preset name in preset selector: ticked by default; unticked, the window's Presets box shows no name.
    #[test]
    fn display_preset_name_switch() {
        let (mut app, dir, _env) = hosted_sampler("shown-name");
        app.state.session.plugins.set_preset(0, Some(dir.join("Rim.ospreset")));
        let entry = |app: &App| {
            let mut m = crate::playlist::plugin_window_menu(0, false, 0.0, 0.0);
            fill_window_menu(app, 0, &mut m);
            m.items.into_iter().find(|i| i.label == "Display preset name in preset selector").unwrap()
        };
        let it = entry(&app);
        assert_eq!((it.mark, it.cmd, it.enabled), (Mark::Check(true), Cmd::Preset(PCmd::ShowName(0, false)), true));
        assert_eq!(app.state.session.plugins.shown_preset(0).as_deref(), Some("Rim"));
        run(&mut app, PCmd::ShowName(0, false));
        assert_eq!(app.state.session.plugins.shown_preset(0), None);
        assert_eq!(app.state.session.plugins.preset_name(0).as_deref(), Some("Rim"), "the preset is still known");
        let it = entry(&app);
        assert_eq!((it.mark, it.cmd), (Mark::Check(false), Cmd::Preset(PCmd::ShowName(0, true))));
        run(&mut app, PCmd::ShowName(0, true));
        assert_eq!(app.state.session.plugins.shown_preset(0).as_deref(), Some("Rim"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Presets box popup's commands: Rename and Delete only for one of the user's presets; rename, delete, import
    /// and browse do what they say.
    #[test]
    fn presets_box_commands() {
        let (mut app, dir, _env) = hosted_sampler("box");
        let h = header(&app, 0).unwrap();
        let enabled = |app: &App, label: &str| popup(app, 0, 0.0, 0.0).items.iter().find(|i| i.label == label).unwrap().enabled;
        assert!(enabled(&app, "Save preset as...") && enabled(&app, "Import preset...") && enabled(&app, "Browse presets"));
        assert!(!enabled(&app, "Rename preset...") && !enabled(&app, "Delete preset..."), "no current preset");
        let state = app.state.session.plugins.state(0).unwrap();
        let mine = file_for(&dir_for(&h.plugin).unwrap(), "Mine");
        save(&mine, &h, &state).unwrap();
        run(&mut app, PCmd::Load(0, mine.clone()));
        assert!(enabled(&app, "Rename preset...") && enabled(&app, "Delete preset..."));
        let renamed = mine.with_file_name("Ours.ospreset");
        rename_chosen(&mut app, 0, mine.clone(), renamed.with_extension(""));
        assert!(!mine.exists() && renamed.exists());
        assert_eq!(app.state.session.plugins.preset_name(0).as_deref(), Some("Ours"));
        assert_eq!(delete_chosen(&mut app, 0, renamed.clone()).as_deref(), Some("Deleted preset Ours"));
        assert!(!renamed.exists());
        assert_eq!(app.state.session.plugins.preset_name(0), None);
        // Import: a preset from elsewhere is copied into the plugin's folder and loads.
        let elsewhere = tmp("box-elsewhere").join("Snare.ospreset");
        save(&elsewhere, &h, &crate::plugins::with_sample_path(&state, "C:/x/snare.wav").unwrap()).unwrap();
        assert_eq!(import_chosen(&mut app, 0, elsewhere.clone()).as_deref(), Some("Imported preset Snare"));
        let copy = file_for(&dir_for(&h.plugin).unwrap(), "Snare");
        assert!(copy.exists());
        assert_eq!(app.state.session.plugins.presets.get(&0), Some(&copy));
        assert_eq!(app.state.session.plugins.sampler_path(0).as_deref(), Some("C:/x/snare.wav"));
        import_chosen(&mut app, 0, elsewhere.clone());
        assert!(copy.with_file_name("Snare 2.ospreset").exists(), "a second import does not replace the first");
        // Browse presets: the browser shows its Plugin presets at the plugin.
        app.state.show_browser = false;
        run(&mut app, PCmd::Browse(0));
        assert!(app.state.show_browser);
        assert_eq!(app.state.browser_sel.as_deref(), Some(format!("{}/Lemon Sampler", crate::views::browser::presets_key()).as_str()));
        let rows = crate::views::browser::rows(&app.state);
        assert!(rows.iter().any(|r| r.label == "Snare" && matches!(&r.item, crate::views::browser::Item::Preset(p) if Path::new(p) == copy)));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(elsewhere.parent().unwrap());
    }

    // spec: ui/settings-choices.md "Miscellaneous: naming, detached plugins and startup"
    /// A preset loaded from the Presets lists names what holds it: a channel while "Name channels after their sample or plugin" is on (the
    /// default), a mixer slot while "Name effect slots after their plugin" is on (off by default). A slot keeps a preset's name until
    /// the next preset or a rename by hand, whatever the check; the slot's window title shows the slot's name.
    #[test]
    fn presets_auto_name_channels_and_effect_slots() {
        use crate::app::App;
        use crate::playlist::EditTarget;
        use crate::prefs::{self, AUTO_NAME_CHANNELS, AUTO_NAME_EFFECT_SLOTS};
        let dir = tmp("auto-name");
        let mut app = App::new(None);
        let mut pl = crate::plugins::Plugins::default();
        pl.available = crate::plugins::builtin();
        let index = pl.index_of(crate::plugins::LEMON_SAMPLER).expect("built in");
        // Rack row 0's instance, and one in Insert 1's first slot, which holds Fruity Delay 3.
        let fx = crate::plugins::fx_key(1, 0);
        pl.create(0, index).unwrap();
        pl.create(fx, index).unwrap();
        app.state.session.plugins = pl;
        app.state.session.project.get_mut().channels[0].name = Some("808 Snare".into());
        crate::mixer::track_mut(&mut app.state.session, 1).unwrap().slots[0] = Some(crate::plugins::new_slot("Fruity Delay 3"));
        let (h, state) = (header(&app, 0).unwrap(), app.state.session.plugins.state(0).unwrap());
        let preset = |name: &str| {
            let p = file_for(&dir, name);
            save(&p, &h, &state).unwrap();
            p
        };
        let (chorus, bandpass, bass) = (preset("Chorus"), preset("Bandpass"), preset("Deep Bass"));
        let channel = |app: &App| app.state.session.project.channels[0].name.clone().unwrap_or_default();
        let slot = |app: &App| crate::mixer::slot_name(&app.state.session, 1, 0).unwrap_or_default();
        // Channels: on by default.
        assert_eq!(run(&mut app, PCmd::Load(0, bass)).as_deref(), Some("Preset: Deep Bass"));
        assert_eq!(channel(&app), "Deep Bass");
        prefs::set_check(&AUTO_NAME_CHANNELS, false);
        app.state.session.project.get_mut().channels[0].name = Some("808 HiHat".into());
        run(&mut app, PCmd::Load(0, chorus.clone()));
        assert_eq!(channel(&app), "808 HiHat", "off: the channel keeps its name");
        // Effect slots: off by default.
        run(&mut app, PCmd::Load(fx, bandpass.clone()));
        // The slot shows the Lemon effect that plays Delay 3 (`plugins::FL_RECREATIONS`).
        assert_eq!(slot(&app), "Lemon Delay", "off: the slot keeps its name");
        prefs::set_check(&AUTO_NAME_EFFECT_SLOTS, true);
        run(&mut app, PCmd::Load(fx, chorus));
        assert_eq!(slot(&app), "Chorus");
        #[cfg(plugin_windows)]
        assert_eq!(crate::mixer::fx_info(&app.state.session.project, fx).unwrap().name, "Chorus", "window title");
        // The name stays with the check off, until a rename by hand or the next preset with it on.
        prefs::set_check(&AUTO_NAME_EFFECT_SLOTS, false);
        run(&mut app, PCmd::Load(fx, bandpass.clone()));
        assert_eq!(slot(&app), "Chorus", "kept");
        crate::playlist::start_edit(&mut app.state, EditTarget::SlotName(1, 0), (0.0, 0.0));
        app.state.playlist.edit.as_mut().unwrap().text = "My echo".into();
        crate::playlist::commit_edit(&mut app.state);
        assert_eq!(slot(&app), "My echo", "renamed by hand");
        prefs::set_check(&AUTO_NAME_EFFECT_SLOTS, true);
        run(&mut app, PCmd::Load(fx, bandpass));
        assert_eq!(slot(&app), "Bandpass", "the next preset");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A parameter-list preset sets the listed parameters and puts the others back to their defaults; names the
    /// plugin does not have are reported.
    #[test]
    fn parameter_presets_load_through_the_parameters() {
        let (mut app, dir, _env) = hosted_sampler("param-load");
        let h = header(&app, 0).unwrap();
        let infos = app.state.session.plugins.param_infos(0);
        let knob = infos.iter().find(|i| !i.read_only && !i.hidden && !i.bypass && i.steps == 0 && i.max > i.min).expect("a knob").clone();
        let mid = knob.min + (knob.max - knob.min) * 0.25;
        let p = dir.join("Quarter.ospreset");
        std::fs::write(&p, encode_params(&h, &[(knob.name.clone(), mid), ("No such knob".into(), 1.0)])).unwrap();
        let msg = run(&mut app, PCmd::Load(0, p)).unwrap();
        assert!(msg.starts_with("Preset: Quarter") && msg.contains("No such knob"), "{msg}");
        assert!((app.state.session.plugins.param_value(0, knob.id).unwrap() - 0.25).abs() < 1e-3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// The user changed the state of the instance under `key` in its own window (host link `Edited`): what changed since
/// its window last knew it (`EditorHost::take_known`) goes to the session, which makes it a step
/// (`presets::window_edit_in`).
#[cfg(plugin_windows)]
pub fn window_edit(app: &mut App, key: usize) {
    let Some(now) = app.state.session.plugins.state(key) else { return };
    let changes = app.state.session.history.changes();
    let edit = app.state.editor_host.take_known(key, &now, changes);
    window_edit_in(&mut app.state.session, key, edit);
}
