//! Open 3x Osc in Lemon Studio: compiled in like Open Sampler, it plays every 3x Osc channel of a project (started from
//! the state the file holds) and every one added from the picker, the browser or the channel rack; saving writes its
//! settings back as the file's 3x Osc state, so the file keeps its own plugin.
//!
// spec: plugins/open-3x-osc.md §6 (app integration), plugins/3x-osc.md §1 (saved state)

use os_model::{Channel, ChannelKind, Project};

use crate::plugins::{Plugins, FX_KEY};

/// The built-in plugin's name (picker, window title).
pub const OPEN_3X_OSC: &str = "Open 3x Osc";
/// The plugin id project files store for it (event 201).
pub const FILE_NAME: &str = "3x Osc";

/// Whether a channel holds the file's 3x Osc.
pub fn is_osc3(ch: &Channel) -> bool {
    ch.kind == ChannelKind::Plugin && ch.plugin.as_deref() == Some(FILE_NAME)
}

/// The plugin id a new channel of picker plugin `name` stores.
pub fn file_name(name: &str) -> &str {
    if name == OPEN_3X_OSC { FILE_NAME } else { name }
}

/// What the UI calls a channel's plugin `name` (the file id of the built-in plugin shows as the built-in plugin).
pub fn label(name: &str) -> &str {
    if name == FILE_NAME { OPEN_3X_OSC } else { name }
}

impl Plugins {
    /// The project-file state of the Open 3x Osc on rack row `row` (its plugin state converted back), `None` when the
    /// row hosts something else.
    pub fn osc3_file_state(&self, row: usize) -> Option<Vec<u8>> {
        let h = self.hosted.get(&row)?;
        let p = h.plugin.lock().ok()?;
        if p.desc().name != OPEN_3X_OSC || !crate::project_plugins::is_builtin(p.desc()) {
            return None;
        }
        open_3x_osc::params::fl_from_plugin_state(&p.save_state()?)
    }
}

/// Before a save: every hosted Open 3x Osc writes its settings into its channel as the file's 3x Osc.
pub fn store(proj: &mut Project, plugins: &Plugins) {
    for row in plugins.hosted.range(..FX_KEY).map(|(&k, _)| k).collect::<Vec<_>>() {
        let Some(st) = plugins.osc3_file_state(row) else { continue };
        if let Some(ch) = proj.channels.get_mut(row).filter(|c| c.kind == ChannelKind::Plugin) {
            ch.plugin = Some(FILE_NAME.into());
            ch.plugin_state = Some(st);
        }
    }
}

impl crate::app::App {
    /// Hosts Open 3x Osc on every 3x Osc channel without an instance, started from the channel's saved state (a new
    /// instance's state when the channel has none). A row waiting for the picker's own instance is left to it; a row
    /// whose instance failed is not tried again.
    pub(crate) fn sync_osc3(&mut self) {
        let Some(index) = self.plugins.index_of(OPEN_3X_OSC) else { return };
        let pending = self.state.pending_plugin.map(|(r, _)| r);
        let rows: Vec<usize> = (0..self.state.project.channels.len())
            .filter(|&r| is_osc3(&self.state.project.channels[r]))
            .filter(|&r| !self.state.hosted_rows.contains(&r) && !self.wrap_failed.contains(&r) && Some(r) != pending)
            .collect();
        if rows.is_empty() {
            return;
        }
        let mut added = false;
        for row in rows {
            let fl = self.state.project.channels[row].plugin_state.clone().unwrap_or_else(open_3x_osc::params::default_state);
            let st = open_3x_osc::params::plugin_state_from_fl(&fl);
            match self.plugins.create_with(row, index, Some(&st)) {
                Ok(_) => {
                    self.state.hosted_rows.push(row);
                    added = true;
                }
                Err(e) => {
                    self.wrap_failed.push(row);
                    self.state.message = Some(format!("{OPEN_3X_OSC}: {e}"));
                }
            }
        }
        if added {
            self.transport.set_instruments(self.plugins.instruments());
            self.state.piano.dirty = true;
        }
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use open_3x_osc::params::{self as op, default_state, field, set_field};
    use os_engine::SharedInstrument;

    fn instance(pl: &Plugins, row: usize) -> SharedInstrument {
        pl.hosted[&row].plugin.clone() as SharedInstrument
    }

    /// The built-in Open 3x Osc starts from a project file's 3x Osc state, plays it, and gives the same bytes back
    /// (spec: plugins/open-3x-osc.md §6).
    #[test]
    fn builtin_open_3x_osc_takes_and_gives_the_file_state() {
        let mut pl = Plugins::default();
        pl.available = crate::plugins::builtin();
        let index = pl.index_of(OPEN_3X_OSC).expect("built-in Open 3x Osc");
        assert!(crate::project_plugins::is_builtin(&pl.available[index].desc));
        let mut fl = default_state();
        set_field(&mut fl, op::osc(0) + op::SHAPE + 1, 3); // osc 1: saw
        set_field(&mut fl, op::MIX2 + 1, 0); // osc 2 and 3 silent
        set_field(&mut fl, op::MIX3 + 1, 0);
        set_field(&mut fl, 109, 77); // a field nothing uses: kept
        pl.create_with(2, index, Some(&op::plugin_state_from_fl(&fl))).unwrap();
        assert_eq!(pl.osc3_file_state(2), Some(fl.clone()));
        // It sounds: a saw at 880 Hz (pitch formula of 3x-osc.md: key 69 with coarse 0), counted at the device rate
        // the instance runs at (44.1 or 48 kHz).
        let inst = instance(&pl, 2);
        let mut i = inst.lock().unwrap();
        i.note_on(0, 69, 100.0 / 127.0);
        let (mut l, mut r) = (vec![0.0f32; 4800], vec![0.0f32; 4800]);
        i.process(48000, &mut l, &mut r);
        let peak = l.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.3 && peak < 1.0, "peak {peak}");
        let crossings = l[480..].windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((77..=89).contains(&crossings), "{crossings} rising crossings in 4320 samples");
        i.note_off(0, 69);
        drop(i);
        // An instance whose plugin is not the built-in gives no file state.
        assert_eq!(pl.osc3_file_state(99), None);
    }

    /// A 3x Osc channel added in the app is saved as the file's 3x Osc with its settings; the reopened project hosts it
    /// again, and a file's 3x Osc channel is hosted from its own state and written back unchanged.
    #[test]
    fn osc3_channels_save_and_reopen() {
        let path = std::env::temp_dir().join(format!("openstudio-osc3-{}.flp", std::process::id()));
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.state.file = None;
        a.state.path = None;
        a.plugins.available = crate::plugins::builtin();
        // Added as the picker adds it: a plugin channel storing the file id, shown with the built-in name.
        let row = crate::app::add_plugin_channel(&mut a.state.project, OPEN_3X_OSC);
        let ch = &a.state.project.channels[row];
        assert_eq!((ch.kind, ch.plugin.as_deref()), (ChannelKind::Plugin, Some(FILE_NAME)));
        assert_eq!(crate::project_plugins::plugin_title(ch).as_deref(), Some(OPEN_3X_OSC));
        // Hosted with a new instance's settings, then one is edited.
        a.sync_osc3();
        assert!(a.state.hosted_rows.contains(&row));
        let mut fl = a.plugins.osc3_file_state(row).unwrap();
        assert_eq!(fl, default_state());
        set_field(&mut fl, op::osc(1) + op::COARSE + 1, 7);
        assert!(a.plugins.set_state(row, &op::plugin_state_from_fl(&fl)));
        a.save_to(path.clone());
        assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved") && !m.contains("not stored")), "{:?}", a.state.message);

        let file = os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap();
        let saved = os_flp::project::load(&file).unwrap();
        let ch = &saved.channels[row];
        assert_eq!((ch.kind, ch.plugin.as_deref()), (ChannelKind::Plugin, Some(FILE_NAME)));
        assert_eq!(ch.plugin_state.as_deref(), Some(fl.as_slice()));
        assert_eq!(field(ch.plugin_state.as_ref().unwrap(), op::osc(1) + op::COARSE + 1), 7);

        // Reopened: hosted again from the file's state.
        let mut b = App::new(None);
        b.plugins.available = crate::plugins::builtin();
        b.open(path.clone());
        b.sync_osc3();
        assert!(b.state.hosted_rows.contains(&row), "{:?}", b.state.message);
        assert_eq!(b.plugins.osc3_file_state(row), Some(fl.clone()));
        // Saved again unedited: the channel's state bytes are unchanged.
        b.save_to(path.clone());
        let again = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&path).unwrap()).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(again.channels[row], saved.channels[row]);
    }

    /// The picker tile and the rack's "Add one ▸" entry add an Open 3x Osc channel and ask for the built-in plugin
    /// (the instance itself is made on the next tick, which also opens its window: done by hand here).
    #[test]
    fn picker_and_rack_add_open_3x_osc_channels() {
        let mut a = App::new(None);
        a.state.project = crate::app::test_project();
        a.plugins.available = crate::plugins::builtin();
        a.state.hosted_generators = vec![OPEN_3X_OSC];
        let n = a.state.project.channels.len();
        a.act(crate::app::Action::PickerTile(false, crate::views::picker::GENERATORS.len()));
        assert_eq!(a.state.project.channels.len(), n + 1);
        assert_eq!(a.state.pending_plugin, Some((n, 0)));
        assert!(is_osc3(&a.state.project.channels[n]));
        assert_eq!(a.state.project.channels[n].name.as_deref(), Some(OPEN_3X_OSC));
        // What the next tick hosts: the picker's entry is the built-in plugin.
        let index = a.plugins.index_of(a.state.hosted_generators[0]).unwrap();
        assert_eq!(a.plugins.available[index].desc.name, OPEN_3X_OSC);
        a.state.pending_plugin = None;
        a.sync_osc3();
        assert!(a.state.hosted_rows.contains(&n));
        assert_eq!(a.plugins.osc3_file_state(n), Some(default_state()));
        // The rack's Add one ▸ entry takes the same path.
        let add = crate::rack::add_one_items(&a.state).into_iter().find(|it| it.label == OPEN_3X_OSC);
        assert!(add.is_some_and(|it| it.cmd == crate::playlist::Cmd::Rack(crate::rack::RCmd::AddPlugin(0))), "Add one lists Open 3x Osc");
        crate::rack::run(&mut a, crate::rack::RCmd::AddPlugin(0));
        assert_eq!(a.state.pending_plugin, Some((n + 1, 0)));
        assert!(is_osc3(&a.state.project.channels[n + 1]));
    }

    /// Corpus check (OPENSTUDIO_FLP_CORPUS): every 3x Osc channel of every file is hosted by Open 3x Osc from its
    /// saved state, gives the same bytes back, and an unedited save leaves the whole project as it was.
    #[test]
    fn corpus_osc3_channels_survive_hosting_and_saving() {
        let Some(list) = std::env::var_os("OPENSTUDIO_FLP_CORPUS") else { return };
        let text = std::fs::read_to_string(list).unwrap();
        let (mut files, mut channels) = (0, 0);
        let out = std::env::temp_dir().join(format!("openstudio-osc3-corpus-{}.flp", std::process::id()));
        for path in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let Some(project) = std::fs::read(path).ok().and_then(|b| os_flp::FlpFile::parse(&b).ok()).and_then(|f| os_flp::project::load(&f).ok())
            else {
                continue;
            };
            let rows: Vec<usize> = (0..project.channels.len()).filter(|&r| is_osc3(&project.channels[r])).collect();
            if rows.is_empty() {
                continue;
            }
            let mut a = App::new(None);
            a.plugins.available = crate::plugins::builtin();
            a.open(std::path::PathBuf::from(path));
            a.sync_osc3();
            for &r in &rows {
                let st = project.channels[r].plugin_state.clone().expect("3x Osc state");
                assert_eq!(st.len(), op::STATE_LEN, "{path}: channel {r}");
                assert_eq!(a.plugins.osc3_file_state(r).as_ref(), Some(&st), "{path}: channel {r}");
                channels += 1;
            }
            let before = a.state.project.clone();
            a.save_to(out.clone());
            assert!(a.state.message.as_deref().is_some_and(|m| m.starts_with("Saved")), "{path}: {:?}", a.state.message);
            let back = os_flp::project::load(&os_flp::FlpFile::parse(&std::fs::read(&out).unwrap()).unwrap()).unwrap();
            for &r in &rows {
                assert_eq!(back.channels[r], before.channels[r], "{path}: channel {r}");
            }
            files += 1;
        }
        let _ = std::fs::remove_file(&out);
        eprintln!("3x Osc: {channels} channels in {files} files");
        assert!(files > 0, "no corpus file with a 3x Osc channel");
    }
}
