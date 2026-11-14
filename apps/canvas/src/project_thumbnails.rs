//! A small picture of each saved project, shown on its card in the start screen's Recent list
//! (as in Studio).
//!
//! The picture is the open document as it was when the project was last saved, scaled down and
//! kept as a PNG in Canvas's settings folder, not in the project file. Each save writes a new
//! file whose name includes the time, because pictures on screen are remembered by their path:
//! a new name is what makes the new picture show.

use std::path::{Path, PathBuf};

/// The longest side of a thumbnail, in pixels: twice the size it is drawn at, for sharp
/// pictures on high-density displays.
pub const MAX_SIDE: u32 = 320;

/// Where thumbnails are kept.
fn folder() -> PathBuf {
    crate::app_settings::folder().join("Thumbnails")
}

/// The start of every thumbnail file's name for the project at `project`: a fingerprint of its
/// path (64-bit FNV-1a, which gives the same value on every run and Rust version), so a
/// project's pictures can be found and replaced without keeping a list.
fn prefix(project: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in project.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The thumbnail files of the project at `project`, oldest first (there is one, or briefly two
/// while a new one replaces the old).
fn files_of(project: &Path) -> Vec<PathBuf> {
    let prefix = prefix(project);
    let Ok(entries) = std::fs::read_dir(folder()) else { return Vec::new() };
    let mut files: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(&prefix))).collect();
    files.sort();
    files
}

/// The picture to show for the project at `project`, if one has been saved: the newest.
pub fn find(project: &Path) -> Option<PathBuf> {
    files_of(project).pop()
}

/// Saves `png` (an already-encoded picture, at most `MAX_SIDE` on its longest side) as the
/// thumbnail of the project at `project`, replacing its earlier one. `now` is seconds since 1970.
pub fn write(project: &Path, png: &[u8], now: u64) -> Result<(), String> {
    if png.is_empty() {
        return Err("there is no picture to save".into());
    }
    let folder = folder();
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let earlier = files_of(project);
    // The time is padded so that names sort oldest to newest.
    let path = folder.join(format!("{}-{now:012}.png", prefix(project)));
    std::fs::write(&path, png).map_err(|error| error.to_string())?;
    for old in earlier.into_iter().filter(|old| *old != path) {
        let _ = std::fs::remove_file(old);
    }
    Ok(())
}

/// Removes the thumbnail of the project at `project`, when it leaves the recent list.
pub fn forget(project: &Path) {
    for file in files_of(project) {
        let _ = std::fs::remove_file(file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thumbnail_is_found_replaced_and_forgotten() {
        let project = std::env::temp_dir().join(format!("thumbnail-test-{}.canvas", std::process::id()));
        forget(&project);
        assert_eq!(find(&project), None);
        write(&project, b"first", 1_000).unwrap();
        let first = find(&project).expect("the thumbnail was saved");
        // A later save replaces it under a new name.
        write(&project, b"second", 2_000).unwrap();
        let second = find(&project).unwrap();
        assert_ne!(first, second);
        assert!(!first.exists());
        assert_eq!(std::fs::read(&second).unwrap(), b"second");
        // Another project's picture is its own.
        assert_eq!(find(&project.with_extension("other")), None);
        forget(&project);
        assert_eq!(find(&project), None);
        assert!(write(&project, b"", 3_000).is_err(), "an empty picture is refused");
    }

    #[test]
    fn the_fingerprint_is_stable() {
        // FNV-1a of the empty string is its starting value; the same path always gives the same name.
        assert_eq!(prefix(Path::new("")), "cbf29ce484222325");
        assert_eq!(prefix(Path::new("/a/b.canvas")), prefix(Path::new("/a/b.canvas")));
        assert_ne!(prefix(Path::new("/a/b.canvas")), prefix(Path::new("/a/c.canvas")));
    }
}
