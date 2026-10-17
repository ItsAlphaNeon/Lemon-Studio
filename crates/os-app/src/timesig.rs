// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! PATTERNS > Set time signature...: the current pattern's own time signature, a time-signature marker at its start,
//! chosen in the "Time signature change" dialog (a `dialog` body), after the conflict question when it has one.
// spec: ui/pattern-time-signature.md

use os_model::TimeMarker;

use crate::app::{Action, App, State};
use crate::dialog::{spans, Answer, Body, DHit, Kind, Msg, Then};
use crate::playlist::{Cmd, Mark, PItem, Popup};

/// What the value lists offer (§1).
pub const NUMERATORS: std::ops::RangeInclusive<u8> = 1..=16;
pub const DENOMINATORS: [u8; 4] = [2, 4, 8, 16];

/// The "Time signature change" dialog: the pattern it is for and the signature chosen so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SigForm {
    pub pattern: u16,
    pub num: u8,
    pub den: u8,
}

/// Index of pattern `n`'s time-signature marker at its start.
pub fn start_marker(s: &State, n: u16) -> Option<usize> {
    s.project.pattern(n)?.markers.iter().position(|m| m.kind == TimeMarker::SIGNATURE && m.position == 0)
}

/// Accept: pattern `n` gets `num` / `den` at its start, first among its markers (they stay sorted by position), named
/// after the signature as FL names it ("3/8", §4-5). The pattern's bars follow it, and so do its clips that kept their
/// natural length.
pub fn set_start(s: &mut State, n: u16, num: u8, den: u8) {
    let old = s.project.pattern_length(n);
    let markers = &mut crate::patterns::pat_mut(s, n).markers;
    markers.retain(|m| !(m.kind == TimeMarker::SIGNATURE && m.position == 0));
    markers.insert(0, TimeMarker { kind: TimeMarker::SIGNATURE, numerator: num, denominator: den, ..TimeMarker::new(0, &format!("{num}/{den}")) });
    crate::playlist::refit_pattern_clips(s, n, old);
}

/// Yes to the conflict question took the start marker away: the pattern's bars are the project's again (§3).
pub fn remove_start(s: &mut State, n: u16) {
    let Some(i) = start_marker(s, n) else { return };
    let old = s.project.pattern_length(n);
    crate::patterns::pat_mut(s, n).markers.remove(i);
    crate::playlist::refit_pattern_clips(s, n, old);
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
    pub(crate) fn ask_time_signature(&mut self) {
        let n = self.state.pattern.max(1);
        if start_marker(&self.state, n).is_none() {
            return self.open_time_signature(n);
        }
        let text = "The pattern already has a time signature defined at the start. Do you want replace it?";
        self.show_msg(Msg::new(Kind::Question, "Time signature conflict", spans(&[text]), &[Answer::Yes, Answer::No], Then::ReplaceSignature(n)));
    }

    /// Yes to the conflict question: the start marker goes at once, then the dialog opens (§3).
    pub(crate) fn replace_signature(&mut self, n: u16) {
        remove_start(&mut self.state, n);
        self.open_time_signature(n);
    }

    /// The dialog for pattern `n`, at 4 / 4 whatever the pattern had (§1).
    fn open_time_signature(&mut self, n: u16) {
        self.state.playlist.popup = None;
        self.show_form(Body::TimeSig(SigForm { pattern: n, num: 4, den: 4 }));
    }

    /// A click on a value box: its list, the app's popup, under the box, the current value marked (§1).
    pub(crate) fn sig_list(&mut self, den: bool) {
        let Some(f) = self.sig_form().copied() else { return };
        let Some(r) = self.hits.iter().rev().find(|h| h.action == Action::Dlg(DHit::SigValue(den))).map(|h| h.rect) else { return };
        let (cur, values): (u8, Vec<u8>) = if den { (f.den, DENOMINATORS.to_vec()) } else { (f.num, NUMERATORS.collect()) };
        let items = values.into_iter().map(|v| PItem::new(v.to_string(), Cmd::TimeSig(den, v)).mark(Mark::Radio(v == cur))).collect();
        self.state.playlist.popup = Some(Popup { x: r.x, y: r.bottom() + 1.0, items, sub: None });
        self.invalidate();
    }

    /// A pick in a value list sets its box.
    pub(crate) fn sig_pick(&mut self, den: bool, v: u8) {
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
