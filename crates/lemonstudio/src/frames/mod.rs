// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The plugin editors' frame windows of each window system (control-plane step 10): the interface draws their chrome
//! (`os_app::plugin_window`) and asks them through `Frames`; these are the windows themselves.

#![allow(unused_imports)]
// Without the `plugin-windows` feature no editor opens in these frames; the window system code stays for the main
// window (focus, transient detached windows).
#![cfg_attr(not(plugin_windows), allow(dead_code))]
pub use os_app::plugin_window::*;
pub use std::f32::consts::PI;
pub use os_hostlink::PageIcon;
pub use os_ui::tabler as tb;
pub use os_ui::{Canvas, Color, Font, Rect};

#[cfg(windows)]
mod win32;
#[cfg(windows)]
pub use win32::{default_position, first_position, focus_main, lift_hide, lift_show, stack, take_exposed, take_keys, take_link, FrameWindow as NativeFrame};

#[cfg(target_os = "linux")]
mod x11;
#[cfg(target_os = "linux")]
mod hyprland;
#[cfg(target_os = "linux")]
pub use x11::{
    busy, default_position, exposed, focus_main, in_keymap, keys_down, lift_hide, lift_show, main_focused, paint_dirty, pointer, pointer_in_docked, pump, screen_to_main,
    set_transient, stack, take_exposed, take_lift_input, unpinned, FrameWindow as NativeFrame,
};

#[cfg(target_os = "macos")]
mod cocoa;
#[cfg(target_os = "macos")]
pub use cocoa::{default_position, lift_hide, lift_show, pointer, row_window_at, screen_to_main, stack, take_exposed, take_keys, take_lift_input, FrameWindow as NativeFrame};

/// Where a detached window opening for the first time goes: where a new window opens. Only Windows places it by
/// monitor (Settings "Open detached plugins with the main window"; spec: ui/settings-choices.md "Miscellaneous:
/// naming, detached plugins and startup").
#[cfg(not(windows))]
pub fn first_position(owner: isize, scale: f32, _with_main: bool) -> (i32, i32) {
    default_position(owner, scale)
}

/// The frames of this platform's window system (`win32`, `x11`, `cocoa`).
#[derive(Default)]
pub struct NativeFrames;

impl Frames for NativeFrames {
    fn open(&self, owner: isize, info: ChannelInfo, editor: (f32, f32), scale: f32, pos: (i32, i32), detached: bool) -> Result<FrameWindow, String> {
        Ok(Box::new(NativeFrame::new(owner, info, editor, scale, pos, detached)?))
    }
    fn default_position(&self, owner: isize, scale: f32) -> (i32, i32) {
        default_position(owner, scale)
    }
    fn first_position(&self, owner: isize, scale: f32, with_main: bool) -> (i32, i32) {
        first_position(owner, scale, with_main)
    }
    fn stack(&self, parent: isize, hwnds: &[isize]) {
        stack(parent, hwnds)
    }
    fn take_exposed(&self) -> bool {
        take_exposed()
    }
    fn take_link(&self) -> Vec<(isize, LinkMsg)> {
        take_link()
    }
    #[cfg(target_os = "linux")]
    fn pump(&self) {
        pump()
    }
    #[cfg(target_os = "linux")]
    fn paint_dirty(&self) {
        paint_dirty()
    }
    #[cfg(target_os = "linux")]
    fn busy(&self) -> bool {
        busy()
    }
    #[cfg(any(windows, target_os = "macos"))]
    fn take_keys(&self) -> Vec<KeyMsg> {
        take_keys()
    }
    #[cfg(target_os = "linux")]
    fn keys_down(&self) -> Option<[u8; 32]> {
        keys_down()
    }
    #[cfg(target_os = "linux")]
    fn in_keymap(&self, keymap: &[u8; 32], code: os_app::keys::KeyCode) -> bool {
        in_keymap(keymap, code)
    }
    fn lift_show(&self, main: isize, frame: isize, origin: (i32, i32), size: (i32, i32), pixels: Vec<u32>) {
        lift_show(main, frame, origin, size, pixels)
    }
    fn lift_hide(&self) {
        lift_hide()
    }
    #[cfg(not(windows))]
    fn take_lift_input(&self) -> Vec<LiftInput> {
        take_lift_input()
    }
    #[cfg(not(windows))]
    fn screen_to_main(&self, x: i32, y: i32) -> Option<(i32, i32)> {
        screen_to_main(x, y)
    }
    #[cfg(not(windows))]
    fn pointer(&self) -> Option<(i32, i32)> {
        pointer()
    }
    #[cfg(target_os = "linux")]
    fn pointer_in_docked(&self) -> bool {
        pointer_in_docked()
    }
    #[cfg(target_os = "macos")]
    fn row_window_at(&self, x: i32, y: i32) -> Option<isize> {
        row_window_at(x, y)
    }
    #[cfg(target_os = "linux")]
    fn main_focused(&self, focused: bool) {
        main_focused(focused)
    }
    #[cfg(target_os = "linux")]
    fn unpinned(&self) -> Unpinned {
        Box::new(unpinned())
    }
    #[cfg(target_os = "linux")]
    fn set_transient(&self, win: u32, owner: isize) {
        set_transient(win, owner)
    }
    #[cfg(any(windows, target_os = "linux"))]
    fn focus_main(&self) {
        focus_main()
    }
}

impl Frame for NativeFrame {
    fn detached(&self) -> bool {
        NativeFrame::detached(self)
    }
    fn set_detached(&self, owner: isize, on: bool) {
        NativeFrame::set_detached(self, owner, on)
    }
    fn clip_to(&self, work: [i32; 4], covers: &[[i32; 4]]) {
        NativeFrame::clip_to(self, work, covers)
    }
    fn hwnd(&self) -> isize {
        NativeFrame::hwnd(self)
    }
    fn area(&self) -> *mut std::ffi::c_void {
        NativeFrame::area(self)
    }
    fn raise(&self) {
        NativeFrame::raise(self)
    }
    fn set_info(&self, info: ChannelInfo) {
        NativeFrame::set_info(self, info)
    }
    fn set_pages(&self, pages: Vec<PageIcon>, active: usize) {
        NativeFrame::set_pages(self, pages, active)
    }
    fn set_resizable(&self, min: Option<(f32, f32)>) {
        NativeFrame::set_resizable(self, min)
    }
    fn set_page(&self, page: usize) {
        NativeFrame::set_page(self, page)
    }
    fn set_hint(&self, hint: String) {
        NativeFrame::set_hint(self, hint)
    }
    fn set_preset(&self, name: Option<String>) {
        NativeFrame::set_preset(self, name)
    }
    fn scale(&self) -> f32 {
        NativeFrame::scale(self)
    }
    fn editor_to_screen(&self, x: f32, y: f32) -> (i32, i32) {
        NativeFrame::editor_to_screen(self, x, y)
    }
    fn set_editor_size(&self, editor: (f32, f32)) {
        NativeFrame::set_editor_size(self, editor)
    }
    fn set_strip(&self, on: bool) {
        NativeFrame::set_strip(self, on)
    }
    fn take_events(&self) -> Vec<FrameEvent> {
        NativeFrame::take_events(self)
    }
    #[cfg(any(windows, target_os = "macos"))]
    fn set_scale(&self, scale: f32) {
        NativeFrame::set_scale(self, scale)
    }
    #[cfg(windows)]
    fn move_into_workspace(&self) {
        NativeFrame::move_into_workspace(self)
    }
    #[cfg(windows)]
    fn move_to(&self, x: i32, y: i32) {
        NativeFrame::move_to(self, x, y)
    }
    #[cfg(windows)]
    fn position(&self) -> Option<(i32, i32)> {
        NativeFrame::position(self)
    }
}

#[cfg(all(test, windows))]
mod win_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// The X server's keymap has a bit per X key code, the evdev scancode winit reads plus 8 (issue #274).
    #[cfg(target_os = "linux")]
    #[test]
    fn keymap_bits_follow_x_key_codes() {
        use os_app::keys::KeyCode;
        let mut map = [0u8; 32];
        // Z: evdev 44, X key code 52.
        map[52 / 8] |= 1 << (52 % 8);
        assert!(in_keymap(&map, KeyCode::KeyZ));
        assert!(!in_keymap(&map, KeyCode::KeyX));
        assert!(!in_keymap(&[0; 32], KeyCode::KeyZ));
    }
}
