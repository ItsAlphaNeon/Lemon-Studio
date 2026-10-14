// Copyright (C) 2026 ItsAlphaNeon. Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio
// SPDX-License-Identifier: GPL-3.0-or-later
//
// This code, in its entirety, is NOT to be used as part of the ArtCraft suite: not this file, not any part of it,
// and not any other part of Lemon Studio.

//! Parameter values by id for an editor, with the editor's own not-yet-confirmed edits on top, and the edit calls that
//! reach both the host and the sound at once.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nih_plug::prelude::*;

use crate::live::LiveEdits;

pub struct Vals {
    pub ptrs: HashMap<String, ParamPtr>,
    overlay: HashMap<String, (f32, Instant)>,
    /// The plugin instance's edits the sound hears (owned by its params object).
    live: Arc<LiveEdits>,
    ctx: Option<Arc<dyn GuiContext>>,
}

impl Vals {
    /// `live` = the edits list of the instance `params` belongs to.
    pub fn new(params: &dyn Params, live: Arc<LiveEdits>, ctx: Option<Arc<dyn GuiContext>>) -> Self {
        Vals { ptrs: params.param_map().into_iter().map(|(id, p, _)| (id, p)).collect(), overlay: HashMap::new(), live, ctx }
    }

    /// The host's value (normalised).
    pub fn actual(&self, id: &str) -> f32 {
        self.ptrs.get(id).map_or(0.0, |p| self.live.norm(*p))
    }

    /// The shown value (normalised): the editor's pending edit, else the host's.
    pub fn norm(&self, id: &str) -> f32 {
        match self.overlay.get(id) {
            Some((v, _)) => *v,
            None => self.actual(id),
        }
    }

    pub fn plain(&self, id: &str) -> f32 {
        let n = self.norm(id);
        // SAFETY: the pointers come from the params object, which outlives the editor.
        self.ptrs.get(id).map_or(0.0, |p| unsafe { p.preview_plain(n) })
    }

    pub fn text(&self, id: &str) -> String {
        self.text_of(id, self.norm(id))
    }

    pub fn text_of(&self, id: &str, norm: f32) -> String {
        self.ptrs.get(id).map_or_else(String::new, |p| unsafe { p.normalized_value_to_string(norm, true) })
    }

    pub fn default_norm(&self, id: &str) -> f32 {
        self.ptrs.get(id).map_or(0.0, |p| unsafe { p.default_normalized_value() })
    }

    pub fn plain_to_norm(&self, id: &str, plain: f32) -> f32 {
        self.ptrs.get(id).map_or(0.0, |p| unsafe { p.preview_normalized(plain) })
    }

    /// Parses a typed value with the parameter's own parser.
    pub fn parse(&self, id: &str, text: &str) -> Option<f32> {
        self.ptrs.get(id).and_then(|p| unsafe { p.string_to_normalized_value(text) })
    }

    /// Drops overlay entries the host has confirmed (or ignored for a second). True when one went.
    pub fn expire(&mut self) -> bool {
        let before = self.overlay.len();
        let now = Instant::now();
        let (ptrs, live) = (&self.ptrs, &self.live);
        self.overlay.retain(|id, (v, t)| {
            let a = ptrs.get(id).map_or(*v, |p| live.norm(*p));
            (a - *v).abs() > 1e-6 && now.duration_since(*t) < Duration::from_secs(1)
        });
        before != self.overlay.len()
    }

    /// A fingerprint of every value (repaint when it changes).
    pub fn signature(&self) -> u64 {
        let mut h: u64 = 1469598103934665603;
        let mut ids: Vec<&String> = self.ptrs.keys().collect();
        ids.sort();
        for id in ids {
            let v = self.norm(id).to_bits();
            h = (h ^ u64::from(v)).wrapping_mul(1099511628211);
        }
        h
    }

    /// Starts a gesture on `id` (drag).
    pub fn begin(&self, id: &str) {
        if let (Some(p), Some(c)) = (self.ptrs.get(id), &self.ctx) {
            unsafe { c.raw_begin_set_parameter(*p) };
        }
    }

    /// Sets `id` inside a gesture: shown at once, heard at once (see [`crate::live`]), and told to the host.
    pub fn set(&mut self, id: &str, norm: f32) {
        let norm = norm.clamp(0.0, 1.0);
        self.overlay.insert(id.to_string(), (norm, Instant::now()));
        if let Some(p) = self.ptrs.get(id) {
            self.live.set(*p, norm);
        }
        if let (Some(p), Some(c)) = (self.ptrs.get(id), &self.ctx) {
            unsafe { c.raw_set_parameter_normalized(*p, norm) };
        }
    }

    pub fn end(&self, id: &str) {
        if let (Some(p), Some(c)) = (self.ptrs.get(id), &self.ctx) {
            unsafe { c.raw_end_set_parameter(*p) };
        }
    }

    /// One complete edit (begin, set, end).
    pub fn edit(&mut self, id: &str, norm: f32) {
        self.begin(id);
        self.set(id, norm);
        self.end(id);
    }

    /// One complete edit by plain value.
    pub fn edit_plain(&mut self, id: &str, plain: f32) {
        let n = self.plain_to_norm(id, plain);
        self.edit(id, n);
    }
}
