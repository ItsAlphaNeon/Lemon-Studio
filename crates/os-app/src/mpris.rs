// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 thacer13
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! MPRIS (Linux): Lemon Studio is a media player to the desktop, on the session bus as
//! `org.mpris.MediaPlayer2.lemonstudio`. Media keys, a bar's player widget and `playerctl` play, pause and stop the
//! transport, and see whether it plays and the project's name. Commands arrive on zbus's thread and wait for the app's
//! tick (`App::mpris_tick`), which wakes for them; the tick tells the bus what changed.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use zbus::zvariant::{ObjectPath, OwnedValue, Value};

const PATH: &str = "/org/mpris/MediaPlayer2";
const NAME: &str = "org.mpris.MediaPlayer2.lemonstudio";

/// What the desktop asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    Play,
    Pause,
    PlayPause,
    Stop,
}

/// What the desktop is shown: "Playing", "Paused" or "Stopped", and the project's name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Now {
    pub status: &'static str,
    pub title: String,
}

/// The player on the bus.
pub struct Mpris {
    conn: zbus::blocking::Connection,
    rx: Receiver<Cmd>,
    now: Arc<Mutex<Now>>,
}

impl Mpris {
    /// On the session bus; `None` without one (or when the name is taken twice over).
    pub fn start() -> Option<Mpris> {
        let (tx, rx) = channel();
        let now = Arc::new(Mutex::new(Now { status: "Stopped", title: String::new() }));
        let player = Player { tx, now: now.clone() };
        let build = |name: String, player: Player| {
            zbus::blocking::connection::Builder::session()?.name(name)?.serve_at(PATH, Root)?.serve_at(PATH, player)?.build()
        };
        // A second Lemon Studio takes an instance name, as MPRIS asks.
        let conn = build(NAME.to_string(), player.clone())
            .or_else(|_| build(format!("{NAME}.instance{}", std::process::id()), player))
            .map_err(|e| eprintln!("MPRIS: {e}"))
            .ok()?;
        Some(Mpris { conn, rx, now })
    }

    /// The commands that came in since the last call.
    pub fn take(&self) -> Vec<Cmd> {
        self.rx.try_iter().collect()
    }

    /// Shows `now`, telling the desktop when it changed.
    pub fn show(&self, now: Now) {
        let old = std::mem::replace(&mut *self.now.lock().unwrap(), now.clone());
        if old == now {
            return;
        }
        let Ok(iface) = self.conn.object_server().interface::<_, Player>(PATH) else { return };
        let p = iface.get();
        let e = iface.signal_emitter();
        if old.status != now.status {
            let _ = zbus::block_on(p.playback_status_changed(e));
        }
        if old.title != now.title {
            let _ = zbus::block_on(p.metadata_changed(e));
        }
    }
}

struct Root;

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    fn raise(&self) {}
    fn quit(&self) {}
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn identity(&self) -> String {
        "Lemon Studio".into()
    }
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

#[derive(Clone)]
struct Player {
    tx: Sender<Cmd>,
    now: Arc<Mutex<Now>>,
}

impl Player {
    fn send(&self, c: Cmd) {
        if self.tx.send(c).is_ok() {
            crate::wake_ui();
        }
    }
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    fn play(&self) {
        self.send(Cmd::Play);
    }
    fn pause(&self) {
        self.send(Cmd::Pause);
    }
    fn play_pause(&self) {
        self.send(Cmd::PlayPause);
    }
    fn stop(&self) {
        self.send(Cmd::Stop);
    }
    fn next(&self) {}
    fn previous(&self) {}
    fn seek(&self, _offset: i64) {}
    fn set_position(&self, _track: ObjectPath<'_>, _position: i64) {}
    fn open_uri(&self, _uri: &str) {}

    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.now.lock().unwrap().status.to_string()
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let title = self.now.lock().unwrap().title.clone();
        let mut m = HashMap::new();
        if let Ok(v) = OwnedValue::try_from(Value::from(ObjectPath::from_static_str_unchecked("/org/lemonstudio/project"))) {
            m.insert("mpris:trackid".to_string(), v);
        }
        if let Ok(v) = OwnedValue::try_from(Value::from(title)) {
            m.insert("xesam:title".to_string(), v);
        }
        if let Ok(v) = OwnedValue::try_from(Value::from(vec!["Lemon Studio".to_string()])) {
            m.insert("xesam:artist".to_string(), v);
        }
        m
    }
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn volume(&self) -> f64 {
        1.0
    }
    #[zbus(property)]
    fn position(&self) -> i64 {
        0
    }
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        false
    }
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Over the bus: the player answers as one, a play-pause reaches the app, what it shows is read back. Needs a
    /// session bus of its own: `dbus-run-session -- cargo test -p os-app mpris -- --ignored` (it would show in the
    /// desktop's player widget on the real one).
    #[test]
    #[ignore]
    fn the_desktop_drives_the_transport() {
        let m = Mpris::start().expect("a session bus");
        m.show(Now { status: "Playing", title: "Song".into() });
        let conn = zbus::blocking::Connection::session().unwrap();
        let p = zbus::blocking::Proxy::new(&conn, NAME, PATH, "org.mpris.MediaPlayer2.Player").unwrap();
        let status: String = p.get_property("PlaybackStatus").unwrap();
        assert_eq!(status, "Playing");
        let meta: HashMap<String, OwnedValue> = p.get_property("Metadata").unwrap();
        assert_eq!(String::try_from(meta["xesam:title"].clone()).unwrap(), "Song");
        let _: () = p.call("PlayPause", &()).unwrap();
        let _: () = p.call("Stop", &()).unwrap();
        assert_eq!(m.take(), vec![Cmd::PlayPause, Cmd::Stop]);
        let root = zbus::blocking::Proxy::new(&conn, NAME, PATH, "org.mpris.MediaPlayer2").unwrap();
        let id: String = root.get_property("Identity").unwrap();
        assert_eq!(id, "Lemon Studio");
    }
}
