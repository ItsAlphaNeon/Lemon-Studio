// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! The pages Lemon Studio opens in the default web browser (Lemon Studio's design): HELP's entries, the toolbar's Help
//! button, the About page's buttons and a plugin's Help. They go to the website's wiki, with what the website leaves
//! out on GitHub: the releases (the site has no changelog of its own and links to them too), bug reports and the
//! source code. Diagnostics writes a report instead (`crate::diagnostics`).

pub const WEBSITE_URL: &str = "https://getlemonstudio.app/";
pub const HELP_INDEX_URL: &str = "https://getlemonstudio.app/wiki/";
pub const SHORTCUTS_URL: &str = "https://getlemonstudio.app/wiki/reference/keyboard-shortcuts/";
pub const TROUBLESHOOTING_URL: &str = "https://getlemonstudio.app/wiki/reference/troubleshooting/";
pub const WHATS_NEW_URL: &str = "https://github.com/ItsAlphaNeon/Lemon-Studio/releases";
pub const REPORT_BUG_URL: &str = "https://github.com/ItsAlphaNeon/Lemon-Studio/issues/new/choose";
pub const SOURCE_CODE_URL: &str = "https://github.com/ItsAlphaNeon/Lemon-Studio";

/// HELP ▸ Tutorials ▸, as (entry, page): the wiki's pages for starting out, in the order it gives them.
pub const TUTORIALS: [(&str, &str); 5] = [
    ("Start here", "https://getlemonstudio.app/wiki/getting-started/"),
    ("Set up your audio", "https://getlemonstudio.app/wiki/getting-started/audio-setup/"),
    ("A tour of the window", "https://getlemonstudio.app/wiki/getting-started/tour/"),
    ("Your first song", "https://getlemonstudio.app/wiki/getting-started/first-song/"),
    ("Command-line rendering", "https://getlemonstudio.app/wiki/reference/command-line-rendering/"),
];

/// The wiki's overview of Lemon Studio's own plugins.
pub const PLUGINS_URL: &str = "https://getlemonstudio.app/wiki/plugins/";

/// The wiki's page on each of Lemon Studio's own plugins that can sit in a mixer slot, as (plugin, page under
/// `PLUGINS_URL`).
const PLUGIN_PAGES: [(&str, &str); 28] = [
    ("Lemon Sampler", "instruments/lemon-sampler"),
    ("Lemon Trio", "instruments/lemon-trio"),
    ("Citrus", "instruments/citrus"),
    ("Smith", "instruments/smith"),
    ("Lemon Slicer", "instruments/lemon-slicer"),
    ("Lemon Drum Sampler", "instruments/lemon-drum-sampler"),
    ("Lemon Parametric EQ", "effects/lemon-parametric-eq"),
    ("Pulp Filter", "effects/pulp-filter"),
    ("Press", "effects/press"),
    ("Lemon Limiter", "effects/lemon-limiter"),
    ("Lemon Reverb", "effects/lemon-reverb"),
    ("Lemon Delay", "effects/lemon-delay"),
    ("Lemon Soft Clipper", "effects/lemon-soft-clipper"),
    ("Lemon Fast Distortion", "effects/lemon-fast-distortion"),
    ("Lemon Balance", "effects/lemon-balance"),
    ("Lemon Stereo Enhancer", "effects/lemon-stereo-enhancer"),
    ("Lemon Transient Processor", "effects/lemon-transient-processor"),
    ("Lemon Chorus", "effects/lemon-chorus"),
    ("Lemon Stereo Shaper", "effects/lemon-stereo-shaper"),
    ("Lemon Filter", "effects/lemon-filter"),
    ("Lemon Flanger", "effects/lemon-flanger"),
    ("Lemon Phaser", "effects/lemon-phaser"),
    ("Lemon Vocoder", "effects/lemon-vocoder"),
    ("Lemon Drop", "effects/lemon-drop"),
    ("Lemon Peak Controller", "controllers/lemon-peak-controller"),
    ("Lemon Formula Controller", "controllers/lemon-formula-controller"),
    ("Tesla", "tools/tesla"),
    ("Lemon Container", "tools/lemon-container"),
];

/// A plugin menu's Help for Lemon Studio's own plugin `name`: its page in the wiki, or the wiki's overview of them if
/// it has none.
pub fn plugin_page(name: &str) -> String {
    match PLUGIN_PAGES.iter().find(|&&(plugin, _)| plugin == name) {
        Some((_, page)) => format!("{PLUGINS_URL}{page}/"),
        None => PLUGINS_URL.to_owned(),
    }
}

/// HELP ▸ Tutorials ▸'s entries.
pub fn tutorial_items() -> Vec<crate::playlist::PItem> {
    TUTORIALS.iter().map(|&(label, url)| crate::playlist::PItem::new(label, crate::playlist::Cmd::Web(url))).collect()
}

/// Opens `url` in the default web browser (tests: nothing).
pub fn open(url: &str) {
    if cfg!(test) {
        return;
    }
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(url).spawn();
}

impl crate::app::App {
    /// Opens `url` in the default web browser, the hint bar saying so.
    pub(crate) fn open_web(&mut self, url: &str) {
        open(url);
        self.state.message = Some(format!("Opening {url}"));
    }
}

#[cfg(test)]
mod tests {
    use crate::views::menus::{item_at, works, HELP_ABOUT, HELP_MENU};

    /// HELP's entries that go online work and open their pages.
    #[test]
    fn help_entries_open_their_pages() {
        let mut app = crate::app::App::new(None);
        let at = |label: &str| (0..20).find(|&i| item_at(HELP_MENU, i).is_some_and(|it| it.label == label)).unwrap();
        for (label, url) in [
            ("Help index", super::HELP_INDEX_URL),
            ("Keyboard shortcuts", super::SHORTCUTS_URL),
            ("Troubleshooting", super::TROUBLESHOOTING_URL),
            ("Report a bug", super::REPORT_BUG_URL),
            ("What's new", super::WHATS_NEW_URL),
            ("Lemon Studio website", super::WEBSITE_URL),
        ] {
            assert!(works(HELP_MENU, item_at(HELP_MENU, at(label)).unwrap()));
            app.menu_item(HELP_MENU, at(label));
            assert_eq!(app.state.message.as_deref(), Some(format!("Opening {url}").as_str()));
        }
    }

    /// Help goes to the website: the wiki is what Help index, the tutorials and a plugin's Help open.
    #[test]
    fn help_goes_to_the_website() {
        let pages = [super::HELP_INDEX_URL, super::SHORTCUTS_URL, super::TROUBLESHOOTING_URL, super::PLUGINS_URL];
        for url in pages.into_iter().chain(super::TUTORIALS.map(|(_, url)| url)) {
            assert!(url.starts_with("https://getlemonstudio.app/wiki/"), "{url}");
        }
    }

    /// Tutorials ▸ works: its submenu lists the wiki's pages for starting out, each opening its page.
    #[test]
    fn tutorials_open_their_pages() {
        use crate::views::menus::submenu;
        let mut app = crate::app::App::new(None);
        let at = (0..20).find(|&i| item_at(HELP_MENU, i).is_some_and(|it| it.label == "Tutorials")).unwrap();
        assert!(works(HELP_MENU, item_at(HELP_MENU, at).unwrap()));
        let items = submenu(HELP_MENU, "Tutorials").unwrap()(&app.state);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_ref()).collect();
        assert_eq!(labels, ["Start here", "Set up your audio", "A tour of the window", "Your first song", "Command-line rendering"]);
        crate::playlist::run(&mut app, items[0].cmd.clone());
        assert_eq!(app.state.message.as_deref(), Some(format!("Opening {}", super::TUTORIALS[0].1).as_str()));
    }

    /// About... opens the settings window on its About page, however it's reached.
    #[test]
    fn about_opens_the_about_page() {
        use crate::views::settings::{ABOUT, TABS};
        let mut app = crate::app::App::new(None);
        assert_eq!(TABS[ABOUT].0, "About");
        assert!(works(HELP_MENU, item_at(HELP_MENU, HELP_ABOUT).unwrap()));
        app.menu_item(HELP_MENU, HELP_ABOUT);
        assert!(app.state.settings.open);
        assert_eq!(app.state.settings.tab, ABOUT);
    }

    /// The About page's buttons open the project's pages.
    #[test]
    fn about_page_buttons_open_the_project_pages() {
        use crate::views::settings::{SAct, SOURCE_CODE, WEBSITE, WIKI};
        let mut app = crate::app::App::new(None);
        for (label, url) in [(WEBSITE, super::WEBSITE_URL), (WIKI, super::HELP_INDEX_URL), (SOURCE_CODE, super::SOURCE_CODE_URL)] {
            app.settings_act(SAct::Button(label));
            assert_eq!(app.state.message.as_deref(), Some(format!("Opening {url}").as_str()));
        }
    }

    /// Every plugin built into Lemon Studio has its page in the wiki; any other name gets the overview. Cutting-edge's
    /// own (Zest) have none: the wiki is main's.
    #[test]
    fn built_in_plugins_have_their_pages() {
        for a in crate::plugins::builtin().iter().chain(&crate::plugins::builtin_effects()).filter(|a| a.desc.name != "Zest") {
            assert_ne!(super::plugin_page(&a.desc.name), super::PLUGINS_URL, "{}", a.desc.name);
        }
        assert_eq!(super::plugin_page(crate::container::CONTAINER), "https://getlemonstudio.app/wiki/plugins/tools/lemon-container/");
        assert_eq!(super::plugin_page("Not a plugin"), super::PLUGINS_URL);
    }
}
