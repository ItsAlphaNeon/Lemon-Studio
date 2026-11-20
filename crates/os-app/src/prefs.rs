// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The declared settings (`os_session::prefs`), read and kept in the interface's store (`crate::settings_file::store`).

pub use os_session::prefs::*;

use crate::settings_file::store;

pub fn slider(s: &Slider) -> u16 {
    store().slider(s)
}

pub fn set_slider(s: &Slider, v: u16) {
    store().set_slider(s, v)
}

pub fn number_text(n: &Number) -> String {
    store().number_text(n)
}

pub fn number(n: &Number) -> u16 {
    store().number(n)
}

pub fn set_number(n: &Number, v: u16) {
    store().set_number(n, v)
}

pub fn check(c: &Check) -> bool {
    store().check(c)
}

pub fn set_check(c: &Check, on: bool) {
    store().set_check(c, on)
}

pub fn toggle(c: &Check) -> bool {
    store().toggle(c)
}

pub fn index(c: &Combo) -> usize {
    store().index(c)
}

pub fn choice(c: &Combo) -> &'static str {
    store().choice(c)
}

pub fn set_choice(c: &Combo, i: usize) {
    store().set_choice(c, i)
}

pub fn text(key: &str) -> Option<String> {
    store().text(key)
}

pub fn set_text(key: &str, value: &str) {
    store().set_text(key, value)
}
