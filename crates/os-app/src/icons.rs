// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Toolbar and window-button icons, and the Lemon Studio logo. Placeholders taken from Tabler Icons (MIT, see `tabler.rs`);
//! each only suggests its function (no artwork is copied from any other program). Where Tabler has nothing that fits,
//! Lemon Studio draws its own on the same grid (`KICK`).

use os_ui::{Canvas, Color, Glyph, Rect};

use os_ui::tabler::{self as tb, TablerIcon};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Minimize,
    Restore,
    Close,
    Play,
    Stop,
    Metronome,
    WaitInput,
    Countdown,
    Blend,
    LoopRecord,
    Undo,
    /// `Undo` mirrored: an undone step of the undo history (the browser's History).
    Redo,
    Cut,
    Mic,
    Help,
    Save,
    SaveRender,
    Info,
    TypingKeyboard,
    AutoScroll,
    StepEdit,
    Slide,
    Link,
    MidiControl,
    Magnet,
    Playlist,
    PianoRoll,
    ChannelRack,
    Mixer,
    Browser,
    Project,
    Plug,
    Kick,
    Touch,
    ArrowRight,
    ArrowUp,
    Back,
    Wave,
    Page,
    Star,
    Folder,
    Search,
    Pencil,
    Brush,
    Delete,
    Mute,
    Slip,
    Slice,
    Select,
    Zoom,
    Speaker,
    Plus,
    Note,
    FolderOpen,
    FolderPlus,
    FileAudio,
    FileProject,
    FileText,
    FilePreset,
    FileZip,
    FileBlank,
}

/// A kick drum seen from the side on its two legs, its pedal's beater against the head (the Tempo tapper). Lemon
/// Studio's own, in Tabler's style: 24-unit grid, outline, absolute M/L/C/Z only.
const KICK: TablerIcon = TablerIcon { filled: false, d: "M 13.5 10.5 C 13.5 14.366 12.381 17.5 11 17.5 C 9.619 17.5 8.5 14.366 8.5 10.5 C 8.5 6.634 9.619 3.5 11 3.5 C 12.381 3.5 13.5 6.634 13.5 10.5 Z M 11 3.5 L 19 3.5 M 11 17.5 L 19 17.5 M 19 3.5 C 20.381 3.5 21.5 6.634 21.5 10.5 C 21.5 14.366 20.381 17.5 19 17.5 M 3 21 L 8 19.5 M 4.8 20.5 L 6.4 12.6 M 8.3 10.8 C 8.3 11.628 7.628 12.3 6.8 12.3 C 5.972 12.3 5.3 11.628 5.3 10.8 C 5.3 9.972 5.972 9.3 6.8 9.3 C 7.628 9.3 8.3 9.972 8.3 10.8 Z M 12 17.3 L 11 21 M 18 17.3 L 19 21" };

/// Tabler's `arrow-back-up` mirrored left to right (an undone step, spec: ui/settings-choices.md "Undo history: the
/// four checks (2026-10-16)").
const REDO: TablerIcon = TablerIcon { filled: false, d: "M 15 14 L 19 10 L 15 6 M 19 10 L 8 10 C 5.791 10 4 11.791 4 14 C 4 16.209 5.791 18 8 18 L 9 18" };

fn shape(icon: Icon) -> &'static TablerIcon {
    match icon {
        Icon::Minimize => &tb::MINUS,
        Icon::Restore => &tb::SQUARES,
        Icon::Close => &tb::X,
        Icon::Play => &tb::PLAYER_PLAY,
        Icon::Stop => &tb::PLAYER_STOP,
        Icon::Metronome => &tb::METRONOME,
        Icon::WaitInput => &tb::HOURGLASS,
        Icon::Countdown => &tb::STOPWATCH,
        Icon::Blend => &tb::BLEND_MODE,
        Icon::LoopRecord => &tb::REPEAT,
        Icon::Undo => &tb::ARROW_BACK_UP,
        Icon::Redo => &REDO,
        Icon::Cut => &tb::CUT,
        Icon::Mic => &tb::MICROPHONE,
        Icon::Help => &tb::HELP,
        Icon::Save => &tb::DEVICE_FLOPPY,
        Icon::SaveRender => &tb::FILE_EXPORT,
        Icon::Info => &tb::INFO_CIRCLE,
        Icon::TypingKeyboard => &tb::KEYBOARD,
        Icon::AutoScroll => &tb::ARROW_AUTOFIT_RIGHT,
        Icon::StepEdit => &tb::GRID_DOTS,
        Icon::Slide => &tb::TRENDING_UP,
        Icon::Link => &tb::LINK,
        Icon::MidiControl => &tb::ADJUSTMENTS_HORIZONTAL,
        Icon::Magnet => &tb::MAGNET,
        Icon::Playlist => &tb::PLAYLIST,
        Icon::PianoRoll => &tb::PIANO,
        Icon::ChannelRack => &tb::LAYOUT_ROWS,
        Icon::Mixer => &tb::ADJUSTMENTS,
        Icon::Browser => &tb::FOLDERS,
        Icon::Project => &tb::FILE,
        Icon::Page => &tb::FILE_TEXT,
        Icon::Plug => &tb::PLUG,
        Icon::Kick => &KICK,
        Icon::Touch => &tb::HAND_FINGER,
        Icon::ArrowRight => &tb::CARET_RIGHT,
        Icon::ArrowUp => &tb::CARET_UP,
        Icon::Back => &tb::ARROW_LEFT,
        Icon::Wave => &tb::WAVE_SINE,
        Icon::Star => &tb::STAR,
        Icon::Folder => &tb::FOLDER,
        Icon::Search => &tb::SEARCH,
        Icon::Pencil => &tb::PENCIL,
        Icon::Brush => &tb::BRUSH,
        Icon::Delete => &tb::ERASER,
        Icon::Mute => &tb::VOLUME_OFF,
        Icon::Slip => &tb::ARROWS_HORIZONTAL,
        Icon::Slice => &tb::SLICE,
        Icon::Select => &tb::MARQUEE_2,
        Icon::Zoom => &tb::ZOOM_IN,
        Icon::Speaker => &tb::VOLUME,
        Icon::Plus => &tb::PLUS,
        Icon::Note => &tb::MUSIC,
        Icon::FolderOpen => &tb::FOLDER_OPEN,
        Icon::FolderPlus => &tb::FOLDER_PLUS,
        Icon::FileAudio => &tb::FILE_MUSIC,
        Icon::FileProject => &tb::FILE_STAR,
        Icon::FileText => &tb::FILE_TYPOGRAPHY,
        Icon::FilePreset => &tb::FILE_SETTINGS,
        Icon::FileZip => &tb::FILE_ZIP,
        Icon::FileBlank => &tb::FILE,
    }
}

/// Draw `icon` centered in `r` with color `c`. The 24-unit Tabler grid is fitted to the shorter side.
pub fn draw(cv: &mut Canvas, icon: Icon, r: Rect, c: Color) {
    cv.icon(shape(icon), r, c);
}

/// Lemon Studio logo (greyscale mark, `logo.rs`) in one tint, fitted into a circle of radius `r`.
pub fn logo(cv: &mut Canvas, cx: f32, cy: f32, r: f32, c: Color) {
    use crate::logo::{PATHS, VIEW_H, VIEW_W};
    let k = 2.0 * r / VIEW_W.max(VIEW_H);
    let (ox, oy) = (cx - VIEW_W * k / 2.0, cy - VIEW_H * k / 2.0);
    let mut g = Glyph::new();
    for d in PATHS {
        g.commands(d, ox, oy, k, None);
    }
    cv.glyph(c, &g);
}
