// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! PATTERNS > Set time signature...: the current pattern's own time signature, a time-signature marker at its start,
//! chosen in the "Time signature change" dialog (a `dialog` body), after the conflict question when it has one. The
//! playlist's time signature markers are chosen in the same dialog.
// spec: ui/pattern-time-signature.md; ui/playlist.md §7a

use os_model::TimeMarker;

use crate::app::{Action, App, State};
use crate::dialog::{spans, Answer, Body, DHit, Kind, Msg, Then};
use crate::playlist::{Cmd, Mark, PItem, Popup};

/// What the value lists offer (§1).
pub const NUMERATORS: std::ops::RangeInclusive<u8> = 1..=16;
pub const DENOMINATORS: [u8; 4] = [2, 4, 8, 16];

/// The "Time signature change" dialog: the pattern it is for and the signature chosen so far; or the song time marker
/// it is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SigForm {
    pub pattern: u16,
    pub num: u8,
    pub den: u8,
    pub song: Option<SongSig>,
}

/// A song time signature marker the dialog is for (spec: ui/playlist.md §7a): a new one at a tick, or marker `i`
/// taking the Time signature action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SongSig {
    At(u32),
    Marker(usize),
}

/// Accept on a song marker: a time signature marker named after its signature ("3/4"), one undo step.
pub fn set_song(s: &mut State, at: SongSig, num: u8, den: u8) {
    crate::edit::playlist::snapshot(&mut s.session);
    let name = format!("{num}/{den}");
    match at {
        SongSig::At(t) => {
            crate::edit::playlist::add_marker(&mut s.session, TimeMarker { kind: TimeMarker::SIGNATURE, numerator: num, denominator: den, ..TimeMarker::new(t, &name) });
        }
        SongSig::Marker(i) => {
            if let Some(m) = crate::edit::playlist::arr_mut(&mut s.session).markers.get_mut(i) {
                *m = TimeMarker { kind: TimeMarker::SIGNATURE, numerator: num, denominator: den, name, ..m.clone() };
            }
        }
    }
    s.session.dirty = true;
}

/// Index of pattern `n`'s time-signature marker at its start.
pub fn start_marker(s: &State, n: u16) -> Option<usize> {
    s.session.project.pattern(n)?.markers.iter().position(|m| m.kind == TimeMarker::SIGNATURE && m.position == 0)
}

/// Accept: pattern `n` gets `num` / `den` at its start, first among its markers (they stay sorted by position), named
/// after the signature as FL names it ("3/8", §4-5). The pattern's bars follow it, and so do its clips that kept their
/// natural length. One undo step.
pub fn set_start(s: &mut State, n: u16, num: u8, den: u8) {
    signature(s, n, num, den);
}

/// Pattern `n`'s signature at its start set (`num` 0: taken away) by its command, one undo step; a pattern number the
/// project has no pattern for gets one.
fn signature(s: &mut State, n: u16, num: u8, den: u8) {
    use crate::control::command::Command;
    if s.session.project.patterns.iter().any(|p| p.id == os_model::Id::NONE) {
        crate::control_ui::settle_ids(s);
    }
    let r = match s.session.project.pattern(n).map(|p| p.id.0) {
        Some(pattern) => crate::control_ui::command(s, STEP, None, vec![Command::SetPatternSignature { pattern, numerator: num, denominator: den }]),
        None => crate::control_ui::command_then(s, STEP, vec![Command::AddPattern { name: None, color: None, number: Some(n) }], |_, r| {
            r.first().and_then(|v| v["pattern"].as_u64()).map(|p| Command::SetPatternSignature { pattern: p as u32, numerator: num, denominator: den }).into_iter().collect()
        }),
    };
    if let Err(e) = r {
        s.message = Some(e);
    }
}

/// The pattern time signature's undo step.
const STEP: &str = "Pattern time signature";


/// Yes to the conflict question took the start marker away: the pattern's bars are the project's again (§3).
pub fn remove_start(s: &mut State, n: u16) {
    if start_marker(s, n).is_none() {
        return;
    }
    signature(s, n, 0, 0);
}

/// The time signature dialog is the top dialog.
pub fn form_open(s: &State) -> bool {
    s.dialogs.last().is_some_and(|m| matches!(m.body, Body::TimeSig(_)))
}

/// The value list open over the dialog: the denominator's (true) or the numerator's.
pub fn list_open(s: &State) -> Option<bool> {
    let p = s.playlist.popup.as_ref().filter(|_| form_open(s))?;
    p.items.iter().find_map(|i| match i.cmd {
        Cmd::TimeSig(den, _) => Some(den),
        _ => None,
    })
}

impl App {
    /// The top dialog's time signature form.
    fn sig_form(&mut self) -> Option<&mut SigForm> {
        match &mut self.state.dialogs.last_mut()?.body {
            Body::TimeSig(f) => Some(f),
            _ => None,
        }
    }

    /// PATTERNS > Set time signature... on the current pattern: the conflict question when it has a signature at its
    /// start (§3), else the dialog.
    pub fn ask_time_signature(&mut self) {
        let n = self.state.pattern().max(1);
        if start_marker(&self.state, n).is_none() {
            return self.open_time_signature(n);
        }
        let text = "The pattern already has a time signature defined at the start. Do you want replace it?";
        self.show_msg(Msg::new(Kind::Question, "Time signature conflict", spans(&[text]), &[Answer::Yes, Answer::No], Then::ReplaceSignature(n)));
    }

    /// Yes to the conflict question: the start marker goes at once, then the dialog opens (§3).
    pub fn replace_signature(&mut self, n: u16) {
        remove_start(&mut self.state, n);
        self.open_time_signature(n);
    }

    /// The dialog for pattern `n`, at 4 / 4 whatever the pattern had (§1).
    fn open_time_signature(&mut self, n: u16) {
        self.state.playlist.popup = None;
        self.show_form(Body::TimeSig(SigForm { pattern: n, num: 4, den: 4, song: None }));
    }

    /// The dialog for a song time signature marker, at 4 / 4 (spec: ui/playlist.md §7a).
    pub fn open_song_signature(&mut self, at: SongSig) {
        self.state.playlist.popup = None;
        let n = self.state.pattern().max(1);
        self.show_form(Body::TimeSig(SigForm { pattern: n, num: 4, den: 4, song: Some(at) }));
    }

    /// A click on a value box: its list, the app's popup, under the box, the current value marked (§1).
    pub fn sig_list(&mut self, den: bool) {
        let Some(f) = self.sig_form().copied() else { return };
        let Some(r) = self.hits.iter().rev().find(|h| h.action == Action::Dlg(DHit::SigValue(den))).map(|h| h.rect) else { return };
        let (cur, values): (u8, Vec<u8>) = if den { (f.den, DENOMINATORS.to_vec()) } else { (f.num, NUMERATORS.collect()) };
        let items = values.into_iter().map(|v| PItem::new(v.to_string(), Cmd::TimeSig(den, v)).mark(Mark::Radio(v == cur))).collect();
        self.state.playlist.popup = Some(Popup { x: r.x, y: r.bottom() + 1.0, items, sub: None, scroll: [0; 2] });
        self.invalidate();
    }

    /// A pick in a value list sets its box.
    pub fn sig_pick(&mut self, den: bool, v: u8) {
        if let Some(f) = self.sig_form() {
            if den {
                f.den = v;
            } else {
                f.num = v;
            }
        }
        self.invalidate();
    }
}
