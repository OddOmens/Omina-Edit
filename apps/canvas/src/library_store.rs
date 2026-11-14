//! Where libraries live on this computer: the same `libraries.json` Studio uses, in Studio's
//! settings folder (macOS: `~/Library/Application Support/Studio`), so a brand, color or text
//! style made in either app shows up in the other. It sits outside every project, so deleting
//! or moving a project never touches it.
//!
//! Both apps may change the file, so every change here reads the file fresh, applies the change
//! to what is on disk and writes it back (`update`), rather than writing out a copy that may be
//! out of date. Canvas also notices when the file changes on disk (`modified`) and reloads.
//!
//! As in Studio, writes go through a temporary file and a rename, so a crash can't leave half a
//! file. Before a change replaces the file, the old one is copied into `Library Backups` (at
//! most one copy every few minutes, and always before something is deleted), and the newest
//! `KEEP_BACKUPS` are kept. A damaged file is set aside and the newest good backup used instead.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use canvas_core::library::Libraries;

/// How many backups are kept.
const KEEP_BACKUPS: usize = 30;
/// Ordinary changes are backed up at most this often (deletions always are).
const BACKUP_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Studio's settings folder, which holds the shared libraries (and Bring to Motion's packages).
pub fn studio_folder() -> PathBuf {
    #[cfg(test)]
    {
        std::env::temp_dir().join(format!("canvas-test-libraries-{}", std::process::id()))
    }
    #[cfg(not(test))]
    {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        if cfg!(target_os = "macos") {
            home.join("Library/Application Support/Studio")
        } else if cfg!(target_os = "windows") {
            std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or(home).join("Studio")
        } else {
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config")).join("Studio")
        }
    }
}

fn library_path() -> PathBuf {
    studio_folder().join("libraries.json")
}

fn backup_folder() -> PathBuf {
    studio_folder().join("Library Backups")
}

fn now_millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_millis())
}

/// Backups, newest first. Their names start with the time they were made, so names sort by age.
fn backups() -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(backup_folder())
        .map(|entries| entries.filter_map(|entry| entry.ok().map(|entry| entry.path())).filter(|path| path.extension().is_some_and(|ext| ext == "json")).collect())
        .unwrap_or_default();
    found.sort();
    found.reverse();
    found
}

fn read(path: &Path) -> Result<Libraries, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    serde_json::from_str::<Libraries>(&text).map_err(|error| error.to_string())?.validated()
}

/// When the libraries file last changed, to notice changes made by Studio. `None` while there
/// is no file.
pub fn modified() -> Option<SystemTime> {
    std::fs::metadata(library_path()).and_then(|metadata| metadata.modified()).ok()
}

/// What loading found: the libraries, and a message for the person if something was repaired.
pub struct Loaded {
    pub libraries: Libraries,
    pub notice: Option<String>,
}

/// Loads the libraries. With no file yet there are none. A damaged file is set aside and the
/// newest good backup used instead.
pub fn load() -> Loaded {
    let path = library_path();
    if !path.exists() {
        return Loaded { libraries: Libraries::default(), notice: None };
    }
    let problem = match read(&path) {
        Ok(libraries) => return Loaded { libraries, notice: None },
        Err(problem) => problem,
    };
    // Keep the damaged file, so nothing is lost even if no backup is good.
    let kept = path.with_file_name(format!("libraries.damaged-{}.json", now_millis()));
    let _ = std::fs::rename(&path, &kept);
    for backup in backups() {
        if let Ok(libraries) = read(&backup) {
            let _ = write(&libraries);
            let notice = format!("Your libraries file was damaged ({problem}), so the latest backup was restored. The damaged file was kept as {}.", kept.display());
            return Loaded { libraries, notice: Some(notice) };
        }
    }
    Loaded { libraries: Libraries::default(), notice: Some(format!("Your libraries file was damaged ({problem}) and no backup could be read. It was kept as {}.", kept.display())) }
}

/// Writes the file, without a backup.
fn write(libraries: &Libraries) -> Result<(), String> {
    std::fs::create_dir_all(studio_folder()).map_err(|error| error.to_string())?;
    let text = serde_json::to_string_pretty(libraries).map_err(|error| error.to_string())?;
    canvas_core::image_io::write_atomically(&library_path(), text.as_bytes()).map_err(|error| format!("could not save the libraries: {error}"))
}

/// Copies the current file into the backups when one is due (or `always`, before a deletion).
fn back_up(always: bool) -> Result<(), String> {
    let path = library_path();
    if !path.exists() {
        return Ok(());
    }
    let latest = backups().first().and_then(|backup| backup.metadata().ok()?.modified().ok());
    let due = latest.is_none_or(|time| time.elapsed().is_ok_and(|age| age >= BACKUP_INTERVAL));
    if always || due {
        std::fs::create_dir_all(backup_folder()).map_err(|error| error.to_string())?;
        std::fs::copy(&path, backup_folder().join(format!("{}.json", now_millis()))).map_err(|error| format!("could not back up the libraries: {error}"))?;
        for old in backups().into_iter().skip(KEEP_BACKUPS) {
            let _ = std::fs::remove_file(old);
        }
    }
    Ok(())
}

/// Makes a change to the libraries as they are on disk now (which Studio may have changed
/// since they were last read), saves the result and returns it with the change's own result.
/// `always_back_up` is for deletions. A file that can't be read is not overwritten.
pub fn update<T>(always_back_up: bool, change: impl FnOnce(&mut Libraries) -> T) -> Result<(Libraries, T), String> {
    let path = library_path();
    let mut libraries = if path.exists() { read(&path).map_err(|problem| format!("The libraries file couldn't be read ({problem}), so the change wasn't saved."))? } else { Libraries::default() };
    let result = change(&mut libraries);
    back_up(always_back_up)?;
    write(&libraries)?;
    Ok((libraries, result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::Rgba;

    /// The tests share one folder, so they run one at a time.
    static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn changes_apply_to_the_file_as_it_is_now_and_back_up_before_deletions() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = std::fs::remove_dir_all(studio_folder());
        let (_, brand) = update(false, |libraries| libraries.add_brand("Brand A")).unwrap();
        assert!(backups().is_empty(), "nothing to back up the first time");
        // Studio adds a brand of its own behind Canvas's back.
        let text = std::fs::read_to_string(library_path()).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value["brands"].as_array_mut().unwrap().push(serde_json::json!({ "id": 3, "name": "From Studio", "colors": [], "text_styles": [] }));
        std::fs::write(library_path(), value.to_string()).unwrap();
        // Canvas's next change keeps it.
        let (libraries, color) = update(false, |libraries| libraries.add_color(brand, "Red", Rgba::from_u32(0xff0000))).unwrap();
        assert!(color.is_some());
        assert_eq!(libraries.brands.iter().map(|brand| brand.name.as_str()).collect::<Vec<_>>(), ["Brand A", "From Studio"]);
        assert_eq!(load().libraries, libraries);

        update(true, |libraries| libraries.delete(brand)).unwrap();
        assert!(!backups().is_empty(), "the brand was backed up before it went");
        let _ = std::fs::remove_dir_all(studio_folder());
    }

    #[test]
    fn a_damaged_file_is_kept_and_the_latest_backup_restored() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = std::fs::remove_dir_all(studio_folder());
        update(false, |libraries| libraries.add_brand("Brand A")).unwrap();
        update(true, |_| ()).unwrap();
        std::fs::write(library_path(), "{ not json").unwrap();
        assert!(update(false, |libraries| libraries.add_brand("B")).is_err(), "a damaged file isn't written over");
        let loaded = load();
        assert_eq!(loaded.libraries.brands.len(), 1, "the backup came back");
        assert!(loaded.notice.is_some_and(|notice| notice.contains("damaged")));
        let kept = std::fs::read_dir(studio_folder()).unwrap().filter_map(Result::ok).any(|entry| entry.file_name().to_string_lossy().starts_with("libraries.damaged-"));
        assert!(kept, "the damaged file was set aside, not deleted");
        let _ = std::fs::remove_dir_all(studio_folder());
    }
}
