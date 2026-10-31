// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Channel states (Lemon Studio's design): a plugin window menu's "Save channel state as..." writes the channel, its
//! settings and its plugin's state, to a file in the user folder's `Channel presets`; "Browse channel states" shows that
//! folder in the browser, and a channel state file dropped (from there or the desktop) adds the channel it holds.
//!
//! A file is a project of the one channel, in the project file format (`os_flp::project::create`), with its own
//! extension: the original's channel state files were not examined, so they are not read and these are not claimed to
//! be theirs.
// spec: ui/plugin-window.md "Channel states", ui/browser.md "Channel presets"

use std::path::{Path, PathBuf};

use crate::app::App;
use os_model::ChannelKind;

/// File extension of a channel state.
pub const EXT: &str = "oschannel";

/// `Channel presets` in the user folder (`OPENSTUDIO_CHANNEL_STATES` overrides it, used by tests).
pub fn root() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("OPENSTUDIO_CHANNEL_STATES") {
        return Some(PathBuf::from(p));
    }
    os_paths::user_file("Channel presets")
}

/// Whether `p` names a channel state file.
pub fn is_state_file(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXT))
}

/// Whether channel `row` can be saved as a channel state: a plugin, sampler or audio clip channel.
pub fn savable(app: &App, row: usize) -> bool {
    app.state.project.channels.get(row).is_some_and(|c| matches!(c.kind, ChannelKind::Plugin | ChannelKind::Sampler | ChannelKind::AudioClip))
}

/// The commands of the plugin window menu's Channel column and the browser.
#[derive(Debug, Clone, PartialEq)]
pub enum CsCmd {
    /// Save channel state as...: the save dialog, for this rack row.
    Save(usize),
    /// Browse channel states: the browser at Channel presets.
    Browse,
    /// A channel state file added as a new channel.
    Add(PathBuf),
}

pub fn run(app: &mut App, c: CsCmd) -> Option<String> {
    match c {
        CsCmd::Save(row) => save_as(app, row),
        CsCmd::Browse => browse(app),
        CsCmd::Add(p) => add(app, &p),
    }
}

/// Save channel state as...: the save dialog in Channel presets, named after the channel (`save_chosen` writes it).
fn save_as(app: &mut App, row: usize) -> Option<String> {
    if !savable(app, row) {
        return Some("This channel has no state to save".into());
    }
    let dir = root()?;
    let _ = std::fs::create_dir_all(&dir);
    let name = crate::presets::folder_name(&crate::rack::channel_name(&app.state, row));
    let dlg = crate::filedlg::FileDlg::new(crate::filedlg::Mode::Save, "Save channel state as", "channel state", crate::dialog::Then::SaveChannelState(row))
        .filter("Lemon Studio channel state", &[EXT])
        .start(Some(&dir))
        .name(&format!("{name}.{EXT}"));
    app.file_dialog(dlg);
    None
}

/// The file chosen in Save channel state as...: channel `row` is written there. Returns the status message.
pub fn save_chosen(app: &mut App, row: usize, target: PathBuf) -> Option<String> {
    let target = if is_state_file(&target) { target } else { target.with_extension(EXT) };
    let shown = target.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Some(match state_bytes(app, row).and_then(|b| std::fs::write(&target, b).map_err(|e| e.to_string())) {
        Ok(()) => {
            app.state.browser_dirs.borrow_mut().clear();
            format!("Saved channel state {shown}")
        }
        Err(e) => format!("Channel state not saved: {e}"),
    })
}

/// Channel `row` as a channel state file: its settings and its plugin's current state, as a save writes them.
pub fn state_bytes(app: &mut App, row: usize) -> Result<Vec<u8>, String> {
    if !savable(app, row) {
        return Err("this channel has no state to save".into());
    }
    // As a project save does: the built-in plugins' and the hosted plugins' states into a copy of the model.
    app.store_builtin_states();
    let mut proj = app.state.project.clone();
    crate::project_plugins::store_hosted(&mut proj, &app.plugins);
    let mut ch = proj.channels.swap_remove(row);
    if ch.kind.is_sampler() {
        let lemon = app.plugins.descriptor(row).is_some_and(|d| d.name == crate::plugins::LEMON_SAMPLER);
        if let Some(st) = app.plugins.state(row).filter(|_| lemon) {
            crate::plugins::store_sampler(&mut ch, &st);
        }
        if let Some(p) = app.plugins.sampler_path(row).filter(|p| !p.is_empty()) {
            if !ch.sample_path.as_deref().is_some_and(|old| crate::plugins::resolve_sample(old) == p) {
                ch.sample_path = Some(p);
            }
        }
    }
    ch.index = 0;
    ch.id = os_model::Id::NONE;
    let one = os_model::Project { ppq: proj.ppq, channels: vec![ch], ..Default::default() };
    os_flp::project::create(&one).map_err(|e| e.to_string())?.to_bytes().map_err(|e| e.to_string())
}

/// The channel a channel state file holds.
pub fn read(path: &Path) -> Result<os_model::Channel, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let file = os_flp::FlpFile::parse(&bytes).map_err(|e| e.to_string())?;
    let p = os_flp::project::load(&file).map_err(|e| e.to_string())?;
    p.channels.into_iter().next().ok_or_else(|| "it holds no channel".into())
}

/// A channel state file dropped or added: its channel becomes the rack's last row, selected, one undo step ("Load
/// channel state"); its plugin starts on the next tick, from the state the file holds. Returns the status message.
pub fn add(app: &mut App, path: &Path) -> Option<String> {
    let shown = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut ch = match read(path) {
        Ok(c) => c,
        Err(e) => return Some(format!("Channel state {shown} not loaded: {e}")),
    };
    ch.id = os_model::Id::NONE;
    if ch.name.as_deref().is_none_or(str::is_empty) {
        ch.name = Some(shown.clone());
    }
    let s = &mut app.state;
    let row = s.project.channels.len();
    crate::history::record(s, LOAD, crate::history::Snap::Channel { row, ch: None });
    crate::rack::push_channel(s, ch);
    s.selected_channel = row;
    s.rack.select_only(row);
    s.piano.dirty = true;
    app.invalidate();
    Some(format!("Loaded channel state {shown}"))
}

/// The undo step of a loaded channel state.
pub const LOAD: &str = "Load channel state";

/// Browse channel states: the browser shows, on its All tab, Channel presets opened (the folder made when missing).
fn browse(app: &mut App) -> Option<String> {
    if let Some(dir) = root() {
        let _ = std::fs::create_dir_all(dir);
    }
    let s = &mut app.state;
    s.show_browser = true;
    s.browser_tab = 0;
    s.browser_search.clear();
    s.browser_dirs.borrow_mut().clear();
    let key = crate::views::browser::channel_presets_key();
    s.browser_open.insert(key.clone());
    crate::browser::select(s, &key);
    crate::browser::reveal(s);
    app.invalidate();
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A channel state round trip (Lemon Studio's design): Save writes the channel, Add brings it back as the last row,
    /// selected, named and set as it was, and is one undo step; Browse opens Channel presets in the browser, listing it.
    #[test]
    fn save_add_and_browse() {
        let dir = std::env::temp_dir().join(format!("ls_channel_states_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: tests touching this variable all set it to the same value.
        unsafe { std::env::set_var("OPENSTUDIO_CHANNEL_STATES", &dir) };
        let mut app = App::new(None);
        app.state.project = crate::app::test_project();
        let row = 1;
        {
            let ch = &mut app.state.project.channels[row];
            ch.name = Some("Snare top".into());
            ch.volume = 0.25;
            ch.misc.cpu.smart_disable = true;
        }
        let path = dir.join("Snare top");
        let m = save_chosen(&mut app, row, path.clone()).unwrap();
        assert_eq!(m, "Saved channel state Snare top");
        let file = path.with_extension(EXT);
        assert!(file.exists());
        let back = read(&file).unwrap();
        assert_eq!((back.name.as_deref(), back.kind, back.volume), (Some("Snare top"), app.state.project.channels[row].kind, 0.25));

        let n = app.state.project.channels.len();
        let m = run(&mut app, CsCmd::Add(file.clone())).unwrap();
        assert_eq!(m, "Loaded channel state Snare top");
        assert_eq!(app.state.project.channels.len(), n + 1);
        let added = &app.state.project.channels[n];
        assert_eq!((added.index as usize, added.name.as_deref(), added.volume), (n, Some("Snare top"), 0.25));
        assert_eq!(app.state.selected_channel, n);
        crate::history::undo(&mut app.state);
        assert_eq!(app.state.project.channels.len(), n, "one undo step");

        assert!(read(&dir.join("missing.oschannel")).is_err());
        run(&mut app, CsCmd::Browse);
        let key = crate::views::browser::channel_presets_key();
        assert!(app.state.show_browser && app.state.browser_open.contains(&key));
        let rows = crate::views::browser::rows(&app.state);
        assert!(rows.iter().any(|r| r.label == "Snare top.oschannel" && r.item == crate::views::browser::Item::Preset(file.to_string_lossy().into_owned())), "{:?}", rows.iter().map(|r| &r.label).collect::<Vec<_>>());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
