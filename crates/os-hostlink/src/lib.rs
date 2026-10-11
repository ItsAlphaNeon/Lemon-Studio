//! Host link: the private conversation between an "Open" plugin (Open Sampler, ...) and OpenStudio's plugin window.
//!
//! OpenStudio draws the frame of every plugin window, including the strip row (page tabs, Misc wrench, Presets, channel
//! controls). A plain VST only gets the "plug" tab. An Open plugin can instead *join* the strip: it tells the host which
//! pages it has (sample, envelope, ...), the host draws their tabs in the same row as the Misc tab and tells the plugin
//! which page is active, and the plugin hides its own tab row. A plugin that never receives the hello (another DAW)
//! keeps drawing its own tabs, so one binary works everywhere.
//!
//! Transport: window messages sent to the plugin's editor window (any plugin format ends up with one on Windows), using
//! one registered message (`MESSAGE_NAME`), `wparam` = [`Cmd`], `lparam` = argument, result = answer. Both sides run on
//! the GUI thread of the same process. Spec: `plugins/host-link.md`. This is an OpenStudio design, not FL's protocol.
// spec: plugins/host-link.md

/// Name passed to `RegisterWindowMessageW`.
pub const MESSAGE_NAME: &str = "OpenStudio.HostLink.1";
/// Protocol version answered to [`Cmd::Hello`].
pub const VERSION: u32 = 1;

/// Commands (the message's `wparam`).
#[repr(usize)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    /// Host -> plugin, `lparam` = host version. The plugin joins the strip and answers its own version (0 = not an Open
    /// plugin; unanswered messages return 0 too).
    Hello = 1,
    /// Answer: number of pages.
    PageCount = 2,
    /// `lparam` = page index; answer: [`PageIcon`] id.
    PageIcon = 3,
    /// Answer: active page index.
    GetPage = 4,
    /// `lparam` = page index to show; answer: the page now active.
    SetPage = 5,
    /// Answer: editor content size in logical px, `width | height << 16` (it shrinks once the plugin hides its tab row).
    Metrics = 6,
    /// `lparam` = pointer to a [`HintBuf`] the plugin fills with the text for the hint line (parameter name and value).
    Hint = 7,
    /// Plugin -> host (sent to the editor's parent window), `lparam` 0: the hint line or the page changed, poll now
    /// instead of waiting for the next tick. Answer 1 when understood.
    Changed = 8,
    /// Plugin -> host: a preview note from the plugin's own keyboard, `lparam` = [`pack_note`]. The host plays it on the
    /// plugin's channel (its live stream) and answers 1; 0 = not handled, the plugin plays it by itself.
    Note = 9,
    /// Plugin -> host: "Create automation clip" for one of the plugin's parameters, `lparam` = its position in the
    /// plugin's parameter list (the CLAP / VST3 parameter index). The host makes the clip for that parameter of this
    /// instance and answers 1; 0 = not handled (a plugin that never got the hello keeps the entry disabled).
    Automate = 10,
}

impl Cmd {
    pub fn from_usize(v: usize) -> Option<Cmd> {
        Some(match v {
            1 => Cmd::Hello,
            2 => Cmd::PageCount,
            3 => Cmd::PageIcon,
            4 => Cmd::GetPage,
            5 => Cmd::SetPage,
            6 => Cmd::Metrics,
            7 => Cmd::Hint,
            8 => Cmd::Changed,
            9 => Cmd::Note,
            10 => Cmd::Automate,
            _ => return None,
        })
    }
}

/// The icon of a page tab. The ids are the protocol; the host maps them to its icon set.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageIcon {
    /// The generic plugin tab (plug).
    Plugin = 0,
    /// Sample / waveform page.
    Sample = 1,
    /// Envelope / modulation page.
    Envelope = 2,
}

impl PageIcon {
    /// Unknown ids fall back to the plugin icon (a newer plugin talking to an older host).
    pub fn from_id(id: u32) -> PageIcon {
        match id {
            1 => PageIcon::Sample,
            2 => PageIcon::Envelope,
            _ => PageIcon::Plugin,
        }
    }
}

/// Text buffer for [`Cmd::Hint`] (UTF-16, `len` code units used).
#[repr(C)]
pub struct HintBuf {
    pub len: u32,
    pub text: [u16; 128],
}

impl Default for HintBuf {
    fn default() -> Self {
        HintBuf { len: 0, text: [0; 128] }
    }
}

impl HintBuf {
    pub fn set(&mut self, s: &str) {
        let mut n = 0;
        for (slot, u) in self.text.iter_mut().zip(s.encode_utf16()) {
            *slot = u;
            n += 1;
        }
        self.len = n as u32;
    }

    pub fn get(&self) -> String {
        String::from_utf16_lossy(&self.text[..(self.len as usize).min(self.text.len())])
    }
}

/// A [`Cmd::Note`] argument: key 0..=127, velocity 0..=127, note on / off.
pub fn pack_note(key: u8, velocity: u8, on: bool) -> isize {
    (key as isize & 0x7f) | ((velocity as isize & 0x7f) << 8) | (isize::from(on) << 16)
}

pub fn unpack_note(v: isize) -> (u8, u8, bool) {
    ((v & 0x7f) as u8, ((v >> 8) & 0x7f) as u8, (v >> 16) & 1 == 1)
}

pub fn pack_size(w: u32, h: u32) -> isize {
    (w.min(0xffff) | (h.min(0xffff) << 16)) as isize
}

pub fn unpack_size(v: isize) -> (u32, u32) {
    let v = v as u32;
    (v & 0xffff, v >> 16)
}

#[cfg(windows)]
mod win {
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetParent, GetWindow, RegisterWindowMessageW, SendMessageTimeoutW, GW_CHILD, SMTO_ABORTIFHUNG};

    use super::*;

    /// The registered window message id.
    pub fn message() -> u32 {
        static ID: OnceLock<u32> = OnceLock::new();
        *ID.get_or_init(|| {
            let w: Vec<u16> = MESSAGE_NAME.encode_utf16().chain(Some(0)).collect();
            unsafe { RegisterWindowMessageW(w.as_ptr()) }
        })
    }

    /// Host side: a connection to the editor window of an Open plugin.
    #[derive(Debug)]
    pub struct Link {
        editor: isize,
        pub pages: Vec<PageIcon>,
        page: usize,
    }

    fn send(hwnd: isize, cmd: Cmd, arg: isize) -> Option<isize> {
        let mut out = 0usize;
        let ok = unsafe { SendMessageTimeoutW(hwnd as HWND, message(), cmd as WPARAM, arg as LPARAM, SMTO_ABORTIFHUNG, 250, &mut out) };
        (ok != 0).then_some(out as isize)
    }

    /// Plugin side: sends a plugin -> host command to the window that holds the editor `editor`. The answer, 0 when the
    /// host does not speak the link (another DAW).
    pub fn to_host(editor: isize, cmd: Cmd, arg: isize) -> isize {
        let parent = unsafe { GetParent(editor as HWND) } as isize;
        if parent == 0 {
            return 0;
        }
        let mut out = 0usize;
        let ok = unsafe { SendMessageTimeoutW(parent as HWND, message(), cmd as WPARAM, arg as LPARAM, SMTO_ABORTIFHUNG, 100, &mut out) };
        if ok != 0 { out as isize } else { 0 }
    }

    impl Link {
        /// Looks for an Open plugin's editor among the children of `area` (the window the host gave the plugin) and
        /// says hello. `None` = an ordinary plugin: show its editor as it is.
        pub fn connect(area: isize) -> Option<Link> {
            let editor = unsafe { GetWindow(area as HWND, GW_CHILD) } as isize;
            if editor == 0 || send(editor, Cmd::Hello, VERSION as isize)? == 0 {
                return None;
            }
            let n = send(editor, Cmd::PageCount, 0)?.clamp(0, 8) as usize;
            let pages = (0..n).map(|i| PageIcon::from_id(send(editor, Cmd::PageIcon, i as isize).unwrap_or(0) as u32)).collect();
            let page = send(editor, Cmd::GetPage, 0).unwrap_or(0).max(0) as usize;
            Some(Link { editor, pages, page })
        }

        /// Editor content size (logical px) after the plugin joined the strip.
        pub fn size(&self) -> Option<(u32, u32)> {
            send(self.editor, Cmd::Metrics, 0).map(unpack_size).filter(|&(w, h)| w > 0 && h > 0)
        }

        /// The active page as last seen.
        pub fn page_now(&self) -> usize {
            self.page
        }

        /// The active page, asked of the plugin (it may change it itself).
        pub fn page(&mut self) -> usize {
            if let Some(p) = send(self.editor, Cmd::GetPage, 0) {
                self.page = p.max(0) as usize;
            }
            self.page
        }

        pub fn set_page(&mut self, i: usize) {
            if let Some(p) = send(self.editor, Cmd::SetPage, i as isize) {
                self.page = p.max(0) as usize;
            }
        }

        /// The plugin's hint line (parameter under the mouse).
        pub fn hint(&self) -> String {
            let mut b = HintBuf::default();
            send(self.editor, Cmd::Hint, &mut b as *mut HintBuf as isize);
            b.get()
        }
    }
}

#[cfg(windows)]
pub use win::{message, to_host, Link};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_and_hint_round_trip() {
        assert_eq!(unpack_size(pack_size(560, 390)), (560, 390));
        let mut b = HintBuf::default();
        b.set("Volume: 50 %");
        assert_eq!(b.get(), "Volume: 50 %");
        assert_eq!(Cmd::from_usize(Cmd::SetPage as usize), Some(Cmd::SetPage));
        assert_eq!(Cmd::from_usize(10), Some(Cmd::Automate));
        assert_eq!(PageIcon::from_id(99), PageIcon::Plugin);
        assert_eq!(unpack_note(pack_note(67, 100, true)), (67, 100, true));
        assert_eq!(unpack_note(pack_note(0, 0, false)), (0, 0, false));
    }
}
