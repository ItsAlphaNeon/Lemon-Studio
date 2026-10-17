// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! `os_host::Library` for FL native plugins, so the app hosts them beside CLAP and VST3.
//!
//! Scanning is cheap: a library records the DLL path and a descriptor derived from the folder
//! layout (`<fl>\Plugins\Fruity\{Generators|Effects}\<Name>\<Name>_x64.dll`) without loading the
//! DLL. The DLL is loaded only when the plugin is created, so a plugin that crashes on load (see
//! spec `plugins/fruity-abi.md`, "Status") does not take down a scan.
//
// spec: plugins/fruity-abi.md

use std::path::{Path, PathBuf};
use std::sync::Arc;

use os_host::{Descriptor, Format, Kind, SharedPlugin};

use crate::adapter::{NativePlugin, MAX_BLOCK};
use crate::Plugin;

/// Rate a native plugin is first loaded at; the engine reactivates it at the real device rate.
const LOAD_RATE: u32 = 44_100;

/// A single FL native plugin found on disk (they are one plugin per DLL).
pub struct NativeLibrary {
    dll: PathBuf,
    fl_dir: PathBuf,
    plugins: Vec<Descriptor>,
}

impl NativeLibrary {
    /// Builds a library for `<...>\<Name>\<Name>_x64.dll`, deriving the FL data folder and the
    /// plugin kind from the path. Does not load the DLL.
    pub fn load(dll: &Path) -> Result<Arc<NativeLibrary>, String> {
        let name = dll
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().into_owned())
            .ok_or_else(|| format!("{}: cannot read plugin folder name", dll.display()))?;
        // .../{kind}/<Name>/<Name>_x64.dll
        let kind_dir = dll.parent().and_then(|p| p.parent()).and_then(|p| p.file_name()).map(|s| s.to_string_lossy().into_owned());
        let is_generator = kind_dir.as_deref() == Some("Generators");
        let kind = if is_generator { Kind::Instrument } else { Kind::Effect };
        // fl_dir = .../<Name>/.. /{kind}/.. /Fruity/.. /Plugins/..
        let fl_dir = dll
            .ancestors()
            .nth(5)
            .ok_or_else(|| format!("{}: not under an FL Plugins folder", dll.display()))?
            .to_path_buf();
        let desc = Descriptor {
            format: Format::Native,
            id: name.clone(),
            name,
            vendor: String::new(),
            version: String::new(),
            features: Vec::new(),
            kind,
            path: dll.to_path_buf(),
        };
        Ok(Arc::new(NativeLibrary { dll: dll.to_path_buf(), fl_dir, plugins: vec![desc] }))
    }
}

impl os_host::Library for NativeLibrary {
    fn path(&self) -> &Path {
        &self.dll
    }

    fn plugins(&self) -> &[Descriptor] {
        &self.plugins
    }

    fn create(self: Arc<Self>, id: &str) -> Result<SharedPlugin, String> {
        let desc = self
            .plugins
            .iter()
            .find(|d| d.id == id)
            .cloned()
            .ok_or_else(|| format!("no native plugin {id}"))?;
        let load = || Plugin::load(&self.dll, &self.fl_dir, LOAD_RATE, MAX_BLOCK);
        let plugin = os_host::guard::catch(load).map_err(|c| os_host::guard::stopped_text(&desc.name, c))?.map_err(|e| e.to_string())?;
        let guard = os_host::guard::module_named(&self.dll);
        Ok(os_host::share(NativePlugin::new(plugin, desc), guard))
    }
}

/// Every installed native plugin DLL under the default FL folder, as library-ready paths.
pub fn dll_files() -> Vec<PathBuf> {
    dll_files_in(&crate::default_fl_dir())
}

/// Every installed native plugin DLL under `fl_dir`.
pub fn dll_files_in(fl_dir: &Path) -> Vec<PathBuf> {
    crate::installed(fl_dir).into_iter().map(|p| p.path).collect()
}
