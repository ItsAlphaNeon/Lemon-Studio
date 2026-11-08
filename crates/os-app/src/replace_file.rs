// Copyright (C) 2026 ItsAlphaNeon
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Saving a file whole or not at all: the new bytes go into a hidden file beside it, reach the disk, and then take the
//! old file's place in one rename. A crash, a full disk or a lost power part-way leaves the old file as it was, never
//! half of the new one.

use std::io::Write;
use std::path::{Path, PathBuf};

/// Puts `bytes` in place of the file at `path` (or creates it). A symlink stays a link: the file it points to is
/// replaced. The old file's permissions carry over.
pub fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let target = if path.is_symlink() { std::fs::canonicalize(path)? } else { path.to_path_buf() };
    let tmp = beside(&target);
    let written = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        if let Ok(m) = std::fs::metadata(&target) {
            std::fs::set_permissions(&tmp, m.permissions())?;
        }
        std::fs::rename(&tmp, &target)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
        return written;
    }
    // The rename itself reaches the disk with the folder (Unix; elsewhere the rename is enough).
    #[cfg(unix)]
    if let Some(dir) = target.parent().and_then(|d| std::fs::File::open(if d.as_os_str().is_empty() { Path::new(".") } else { d }).ok()) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// The hidden file beside `target` the new bytes go into first: its name starts with a dot and does not end in the
/// project's extension, so neither the browser nor a file dialog lists it.
fn beside(target: &Path) -> PathBuf {
    let name = target.file_name().map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
    target.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lemon-replace-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The new bytes take the old ones' place, and nothing else is left in the folder.
    #[test]
    fn a_file_is_replaced_whole() {
        let d = dir("whole");
        let p = d.join("Song.lemon");
        replace(&p, b"first").unwrap();
        replace(&p, b"second, longer").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"second, longer");
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1, "no file left beside it");
        let _ = std::fs::remove_dir_all(d);
    }

    /// A write that cannot be made leaves the old file as it was, and no file beside it.
    #[test]
    fn a_failed_write_leaves_the_old_file() {
        let d = dir("failed");
        let p = d.join("Song.lemon");
        replace(&p, b"old").unwrap();
        // The target is a folder now: the rename over it fails after the new bytes were written.
        let q = d.join("Folder.lemon");
        std::fs::create_dir(&q).unwrap();
        std::fs::write(q.join("inside"), b"x").unwrap();
        assert!(replace(&q, b"new").is_err());
        assert_eq!(std::fs::read(&p).unwrap(), b"old");
        assert!(!std::fs::read_dir(&d).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().ends_with(".tmp")), "nothing left beside it");
        let _ = std::fs::remove_dir_all(d);
    }

    /// A symlink stays a link to the file it points to, which gets the new bytes; the file's permissions stay.
    #[cfg(unix)]
    #[test]
    fn a_link_and_the_permissions_stay() {
        use std::os::unix::fs::PermissionsExt;
        let d = dir("link");
        let real = d.join("real.lemon");
        std::fs::write(&real, b"old").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
        let link = d.join("link.lemon");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        replace(&link, b"new").unwrap();
        assert!(link.is_symlink(), "still a link");
        assert_eq!(std::fs::read(&real).unwrap(), b"new");
        assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o640);
        let _ = std::fs::remove_dir_all(d);
    }
}
