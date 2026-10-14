//! Toolbar and window-button icons, and the Lemon Studio logo. Placeholders taken from Tabler Icons (MIT, see `tabler.rs`);
//! each only suggests its function (no artwork is copied from any other program).

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
    Cut,
    Mic,
    Help,
    Save,
    SaveRender,
    Info,
    Download,
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
    AudioEditor,
    Touch,
    Store,
    Globe,
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
        Icon::Cut => &tb::CUT,
        Icon::Mic => &tb::MICROPHONE,
        Icon::Help => &tb::HELP,
        Icon::Save => &tb::DEVICE_FLOPPY,
        Icon::SaveRender => &tb::FILE_EXPORT,
        Icon::Info => &tb::INFO_CIRCLE,
        Icon::Download => &tb::DOWNLOAD,
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
        Icon::AudioEditor => &tb::WAVE_SINE,
        Icon::Touch => &tb::HAND_FINGER,
        Icon::Store => &tb::SHOPPING_BAG,
        Icon::Globe => &tb::WORLD,
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
