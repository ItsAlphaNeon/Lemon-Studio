// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Session restore: closing costs nothing. With General ▸ Miscellaneous ▸ Startup project on "Last used project"
//! (cutting-edge's default), a start opens the project the app closed with and opens again the windows that were open
//! in it, back to front: the arranger, mixer, piano roll, instrument rack, the channel window (on its channel), the tool
//! windows and the plugin windows. A plugin's window opens once its plugin has loaded (`App::session_tick`). Where
//! the windows were is `layout`'s, as before.
//!
//! Kept as the app closes, in `settings.txt`: `session_project` (the project's file) and `session_windows` (tokens,
//! back to front: a window's name, `channel:<row>` for the channel window, `plugin:<key>` for a plugin's window by its
//! hosted key, a rack row or a mixer slot's `plugins::fx_key`). The windows come back only with that same project.

use std::path::PathBuf;

use crate::app::{App, Win};
use crate::settings_file::{get, write};

const PROJECT: &str = "session_project";
const WINDOWS: &str = "session_windows";

/// How long a plugin's window waits for its plugin to load before it is given up.
const WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// The windows kept by name.
const NAMES: [(Win, &str); 9] = [
    (Win::Playlist, "arranger"),
    (Win::Mixer, "mixer"),
    (Win::PianoRoll, "piano-roll"),
    (Win::ChannelRack, "instrument-rack"),
    (Win::Perf, "plugin-performance"),
    (Win::Tapper, "tempo-tapper"),
    (Win::Touch, "touch-controller"),
    (Win::Strum, "strum"),
    (Win::Arp, "arpeggiate"),
];

/// What a start puts back.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Win(Win),
    Channel(usize),
    Plugin(usize),
}

fn parse(t: &str) -> Option<Token> {
    if let Some(n) = t.strip_prefix("channel:") {
        return n.parse().ok().map(Token::Channel);
    }
    if let Some(n) = t.strip_prefix("plugin:") {
        return n.parse().ok().map(Token::Plugin);
    }
    NAMES.iter().find(|(_, n)| *n == t).map(|(w, _)| Token::Win(*w))
}

fn text(t: &Token) -> String {
    match t {
        Token::Win(w) => NAMES.iter().find(|(x, _)| x == w).map_or(String::new(), |(_, n)| n.to_string()),
        Token::Channel(r) => format!("channel:{r}"),
        Token::Plugin(k) => format!("plugin:{k}"),
    }
}

/// The open windows of `app`, back to front.
pub fn tokens(app: &App) -> Vec<Token> {
    let s = &app.state;
    s.z.iter()
        .filter_map(|&w| match w {
            Win::Plugin => Some(Token::Channel(s.plugin_channel)),
            Win::Hosted(k) => app.plugins.hosted.get(&k).filter(|h| h.window.is_some()).map(|_| Token::Plugin(k)),
            w => NAMES.iter().any(|(x, _)| *x == w).then_some(Token::Win(w)),
        })
        .chain(
            // Plugin windows of their own (not in the window stack): after the rest.
            app.plugins.hosted.iter().filter(|(k, h)| h.window.is_some() && !s.z.contains(&Win::Hosted(**k))).map(|(k, _)| Token::Plugin(*k)),
        )
        .collect()
}

/// The app closes: its project and open windows, for the next start.
pub fn save(app: &App) {
    let path = app.state.path.as_ref().map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()));
    write(PROJECT, &path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
    write(WINDOWS, &tokens(app).iter().map(text).collect::<Vec<_>>().join(" "));
}

/// The windows kept for the project now open (`None`: another project, or nothing kept).
pub fn kept(app: &App) -> Option<Vec<Token>> {
    let project = PathBuf::from(get(PROJECT)?);
    let now = app.state.path.as_ref().map(|p| std::path::absolute(p).unwrap_or_else(|_| p.clone()))?;
    (project == now).then(|| get(WINDOWS).unwrap_or_default().split_whitespace().filter_map(parse).collect())
}

impl App {
    /// After the start's project opened: its windows as they were (`kept`); the plugin windows wait for their plugins.
    pub(crate) fn restore_session(&mut self) {
        let Some(tokens) = kept(self) else { return };
        let rows = self.state.project.channels.len();
        for t in tokens {
            match t {
                Token::Win(w) => self.state.raise(w),
                Token::Channel(r) if r < rows => self.open_channel_window(r),
                Token::Channel(_) => {}
                Token::Plugin(k) => self.session_plugins.push(k),
            }
        }
        self.session_since = Some(std::time::Instant::now());
        self.invalidate();
    }

    /// The kept plugin windows whose plugins have loaded open, in the order kept; the rest wait (`WAIT` at most).
    pub(crate) fn session_tick(&mut self) {
        let Some(since) = self.session_since else { return };
        if since.elapsed() > WAIT {
            self.session_plugins.clear();
        }
        while let Some(&k) = self.session_plugins.first() {
            if !self.plugins.hosted.contains_key(&k) {
                break;
            }
            self.session_plugins.remove(0);
            match crate::plugins::fx_of(k) {
                Some((track, slot)) => self.open_fx(track, slot),
                None => self.state.pending_open = Some(k),
            }
            // One a tick: `pending_open` holds one.
            break;
        }
        if self.session_plugins.is_empty() {
            self.session_since = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_read_back() {
        for t in [Token::Win(Win::Mixer), Token::Channel(3), Token::Plugin(crate::plugins::fx_key(2, 1)), Token::Win(Win::ChannelRack)] {
            assert_eq!(parse(&text(&t)), Some(t));
        }
        assert_eq!(parse("browser"), None);
    }

    /// The windows open when the app closed come back with the same project, back to front; another project gets its
    /// usual ones.
    #[test]
    fn windows_come_back_with_their_project() {
        let dir = std::env::temp_dir().join(format!("lemon-session-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let song = dir.join("Song.flp");
        let mut a = App::new(None);
        a.save_to(song.clone());
        a.state.raise(Win::Mixer);
        a.state.raise(Win::Tapper);
        a.state.raise(Win::ChannelRack);
        save(&a);
        let before = tokens(&a);
        let mut b = App::new(None);
        b.open(song.clone());
        b.state.hide(Win::Mixer);
        b.state.hide(Win::Tapper);
        b.restore_session();
        let order: Vec<Win> = b.state.z.iter().copied().filter(|w| matches!(w, Win::Mixer | Win::Tapper | Win::ChannelRack)).collect();
        assert_eq!(order, [Win::Mixer, Win::Tapper, Win::ChannelRack], "back to front");
        assert_eq!(tokens(&b), before);
        let mut c = App::new(None);
        c.save_to(dir.join("Other.flp"));
        assert_eq!(kept(&c), None, "another project");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
