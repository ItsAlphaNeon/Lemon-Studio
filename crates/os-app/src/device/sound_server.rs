// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Linux: gives the sound server back the cards Lemon Studio had through ALSA (exclusive mode).
//!
//! While Lemon Studio has an ALSA device open, the sound server (PipeWire, PulseAudio) cannot: opening the card fails
//! as "busy", and PipeWire's session manager gives up on the card's speakers and microphone for good. They do not
//! come back when Lemon Studio lets go, or when it exits, until the session manager restarts or the computer does.
//! Choosing the card's profile again (off, then the one it had) makes the server open the card anew.
//!
//! `heal` does that for each card whose profile has an output (an input) and that has no sink (no source) on the
//! server: one it gave up on. Other cards, and programs playing on them, are left alone. It speaks the PulseAudio
//! protocol, which PipeWire serves too (pipewire-pulse); without a sound server it does nothing.

use std::ffi::CString;
use std::io::BufReader;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use pulseaudio::protocol::{self, Command};

/// Waits for the server's answers at most this long: a server that hangs does not hold up the audio thread or exit.
const TIMEOUT: Duration = Duration::from_secs(2);

/// Chooses again the profile of each card the server gave up on; returns those cards' names.
pub(crate) fn heal() -> Vec<String> {
    match try_heal() {
        Ok(healed) => healed,
        Err(e) => {
            eprintln!("sound server: couldn't check its cards: {e}");
            Vec::new()
        }
    }
}

/// A card's profile that plays (records) needs a sink (a source other than a monitor) of that card on the server.
fn lost(profile: &str, sinks: bool, sources: bool) -> bool {
    profile != "off" && ((profile.contains("output") && !sinks) || (profile.contains("input") && !sources))
}

struct Conn {
    sock: BufReader<UnixStream>,
    version: u16,
    seq: u32,
}

impl Conn {
    fn open() -> Result<Option<Conn>, Box<dyn std::error::Error>> {
        let Some(path) = pulseaudio::socket_path_from_env() else { return Ok(None) };
        let Ok(stream) = UnixStream::connect(path) else { return Ok(None) };
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        let mut c = Conn { sock: BufReader::new(stream), version: protocol::MAX_VERSION, seq: 0 };
        let cookie = pulseaudio::cookie_path_from_env().and_then(|p| std::fs::read(p).ok()).unwrap_or_default();
        let auth = protocol::AuthParams { version: protocol::MAX_VERSION, supports_shm: false, supports_memfd: false, cookie };
        let reply: protocol::AuthReply = c.ask(Command::Auth(auth))?;
        c.version = c.version.min(reply.version);
        let mut props = protocol::Props::new();
        props.set(protocol::Prop::ApplicationName, c"Lemon Studio");
        let _: protocol::SetClientNameReply = c.ask(Command::SetClientName(props))?;
        Ok(Some(c))
    }

    fn send(&mut self, cmd: &Command) -> Result<u32, protocol::ProtocolError> {
        let seq = self.seq;
        self.seq += 1;
        protocol::write_command_message(self.sock.get_mut(), seq, cmd, self.version)?;
        Ok(seq)
    }

    fn ask<R: protocol::CommandReply>(&mut self, cmd: Command) -> Result<R, protocol::ProtocolError> {
        self.send(&cmd)?;
        Ok(protocol::read_reply_message::<R>(&mut self.sock, self.version)?.1)
    }

    fn act(&mut self, cmd: Command) -> Result<(), protocol::ProtocolError> {
        self.send(&cmd)?;
        protocol::read_ack_message(&mut self.sock).map(|_| ())
    }
}

fn try_heal() -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let Some(mut c) = Conn::open()? else { return Ok(Vec::new()) };
    let cards: protocol::CardInfoList = c.ask(Command::GetCardInfoList)?;
    let sinks: protocol::SinkInfoList = c.ask(Command::GetSinkInfoList)?;
    let sources: protocol::SourceInfoList = c.ask(Command::GetSourceInfoList)?;
    let mut healed = Vec::new();
    for card in cards {
        let Some(profile) = card.active_profile.clone() else { continue };
        let has_sink = sinks.iter().any(|s| s.card_index == Some(card.index));
        let has_source = sources.iter().any(|s| s.card_index == Some(card.index) && s.monitor_of_sink_index.is_none());
        if !lost(&profile.to_string_lossy(), has_sink, has_source) {
            continue;
        }
        let set = |p: CString| Command::SetCardProfile(protocol::SetCardProfileParams { card_index: Some(card.index), card_name: None, profile_name: p });
        c.act(set(c"off".into()))?;
        c.act(set(profile))?;
        healed.push(card.name.to_string_lossy().into_owned());
    }
    Ok(healed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only a card whose profile plays (records) with no sink (source) on the server is chosen again.
    #[test]
    fn which_cards_were_given_up_on() {
        let duplex = "output:analog-stereo+input:analog-stereo";
        assert!(lost(duplex, false, true), "the speakers are gone");
        assert!(lost(duplex, true, false), "the microphone is gone");
        assert!(!lost(duplex, true, true));
        assert!(!lost("output:hdmi-stereo", true, false), "no input in the profile");
        assert!(!lost("off", false, false));
        assert!(!lost("pro-audio", false, false), "a profile naming neither is left alone");
    }

    /// The cards this computer's sound server gave up on are given back, and checking again finds none. Needs a sound
    /// server (it may choose a card's profile again: `--ignored` only).
    #[test]
    #[ignore]
    fn heals_the_cards_here() {
        println!("given back: {:?}", heal());
        std::thread::sleep(Duration::from_secs(1));
        assert_eq!(heal(), Vec::<String>::new(), "none is left");
    }
}
