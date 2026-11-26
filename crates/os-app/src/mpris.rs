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

/// The player on the bus. All blocking D-Bus connection setup and signal emissions run on a dedicated background
/// thread so a slow or unavailable session bus never stalls app startup or the UI tick.
pub struct Mpris {
    rx: Receiver<Cmd>,
    tx: Sender<Now>,
    last: Mutex<Now>,
    #[cfg(test)]
    ready: Arc<std::sync::atomic::AtomicBool>,
}

impl Mpris {
    /// Starts the player on the session bus in a background thread.
    pub fn start() -> Option<Mpris> {
        let (cmd_tx, cmd_rx) = channel();
        let (upd_tx, upd_rx) = channel::<Now>();
        let shared = Arc::new(Mutex::new(Now { status: "Stopped", title: String::new() }));
        let player_now = shared.clone();
        #[cfg(test)]
        let ready = Arc::new(std::sync::atomic::AtomicBool::new(false));
        #[cfg(test)]
        let ready_bg = ready.clone();
        std::thread::Builder::new()
            .name("lemonstudio-mpris".into())
            .spawn(move || {
                let player = Player { tx: cmd_tx, now: player_now };
                let build = |name: String, player: Player| {
                    zbus::blocking::connection::Builder::session()?.name(name)?.serve_at(PATH, Root)?.serve_at(PATH, player)?.build()
                };
                // A second Lemon Studio takes an instance name, as MPRIS asks.
                let Ok(conn) = build(NAME.to_string(), player.clone())
                    .or_else(|_| build(format!("{NAME}.instance{}", std::process::id()), player))
                    .map_err(|e| eprintln!("MPRIS: {e}"))
                else {
                    return;
                };
                #[cfg(test)]
                ready_bg.store(true, std::sync::atomic::Ordering::Release);
                while let Ok(mut now) = upd_rx.recv() {
                    // Coalesce burst updates so the bus only emits the newest state.
                    while let Ok(newer) = upd_rx.try_recv() {
                        now = newer;
                    }
                    let (status_changed, title_changed) = {
                        let mut cur = shared.lock().unwrap_or_else(|e| e.into_inner());
                        let sc = cur.status != now.status;
                        let tc = cur.title != now.title;
                        if sc || tc {
                            *cur = now;
                        }
                        (sc, tc)
                    };
                    if !status_changed && !title_changed {
                        continue;
                    }
                    let Ok(iface) = conn.object_server().interface::<_, Player>(PATH) else { continue };
                    let p = iface.get();
                    let e = iface.signal_emitter();
                    if status_changed {
                        let _ = zbus::block_on(p.playback_status_changed(e));
                    }
                    if title_changed {
                        let _ = zbus::block_on(p.metadata_changed(e));
                    }
                }
            })
            .ok()?;
        Some(Mpris {
            rx: cmd_rx,
            tx: upd_tx,
            last: Mutex::new(Now { status: "Stopped", title: String::new() }),
            #[cfg(test)]
            ready,
        })
    }

    /// The commands that came in since the last call.
    pub fn take(&self) -> Vec<Cmd> {
        self.rx.try_iter().collect()
    }

    /// Shows `status` and `title`, telling the desktop only when either changed.
    pub fn show(&self, now: Now) {
        self.show_str(now.status, &now.title);
    }

    /// Like `show`, avoiding `String` allocation and D-Bus work when unchanged.
    pub fn show_str(&self, status: &'static str, title: &str) {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if last.status == status && last.title == title {
            return;
        }
        last.status = status;
        last.title.clear();
        last.title.push_str(title);
        let _ = self.tx.send(last.clone());
    }
}

impl crate::app::App {
    /// MPRIS (`crate::mpris`): the desktop's play, pause and stop, as the transport's buttons; then what it shows.
    pub(crate) fn mpris_tick(&mut self) {
        let Some(m) = &self.mpris else { return };
        for c in m.take() {
            let playing = self.state.session.transport.state() == crate::transport::State::Playing;
            match c {
                Cmd::Play if playing => {}
                Cmd::Pause if !playing => {}
                Cmd::Play | Cmd::Pause | Cmd::PlayPause => self.act(crate::app::Action::Play),
                Cmd::Stop => self.act(crate::app::Action::Stop),
            }
        }
        let status = match self.state.session.transport.state() {
            crate::transport::State::Playing => "Playing",
            crate::transport::State::Paused => "Paused",
            crate::transport::State::Stopped => "Stopped",
        };
        let title = self.title();
        let trimmed = title.trim_start_matches('[').trim_end_matches(']');
        if let Some(m) = &self.mpris {
            m.show_str(status, trimmed);
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
        self.now.lock().unwrap_or_else(|e| e.into_inner()).status.to_string()
    }
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        let title = self.now.lock().unwrap_or_else(|e| e.into_inner()).title.clone();
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
        let t0 = std::time::Instant::now();
        while !m.ready.load(std::sync::atomic::Ordering::Acquire) && t0.elapsed() < std::time::Duration::from_secs(2) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        m.show(Now { status: "Playing", title: "Song".into() });
        std::thread::sleep(std::time::Duration::from_millis(20));
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
