//! Crash reports, as Omina Motion keeps them. If Omina Edit crashes, what went wrong (the
//! error, where in the code, and the chain of calls that led there) is written to a text file in
//! the settings folder's `Crash Reports` folder. On the next launch Omina Edit says it closed
//! unexpectedly and offers to show the report, so it can be sent along with a bug report.
//! Nothing is sent anywhere.

use std::path::PathBuf;

use gpui_kit::component::WindowExt as _;
use gpui_kit::{Context, Window};

use crate::workspace::Workspace;

/// The folder crash reports are written to.
pub fn folder() -> PathBuf {
    crate::app_settings::folder().join("Crash Reports")
}

/// Names the report not yet shown to the user, if there is one.
fn unseen_marker() -> PathBuf {
    folder().join(".unseen")
}

/// Writes a report whenever Omina Edit panics, then lets the usual handling go on (printing
/// the error on the terminal). Called once, first thing in `main`.
pub fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".to_owned());
        let location = info.location().map_or_else(|| "unknown".to_owned(), |place| format!("{}:{}", place.file(), place.line()));
        let thread = std::thread::current().name().unwrap_or("unnamed").to_owned();
        let backtrace = std::backtrace::Backtrace::force_capture();
        let report = report_text(&message, &location, &thread, &backtrace.to_string());
        if let Err(error) = save_report(&report) {
            eprintln!("could not save a crash report: {error}");
        }
        previous(info);
    }));
}

/// The report's text: what crashed, where, on which version and system, and the calls that led
/// there.
fn report_text(message: &str, location: &str, thread: &str, backtrace: &str) -> String {
    format!(
        "{name} {version} crashed\nWhen: {when} (seconds since 1970)\nSystem: {os} {arch}\nThread: {thread}\nWhere: {location}\nError: {message}\n\nCalls leading to it:\n{backtrace}\n",
        name = canvas_core::names::EDIT,
        version = env!("CARGO_PKG_VERSION"),
        when = crate::app_settings::now_seconds(),
        os = std::env::consts::OS,
        arch = std::env::consts::ARCH,
    )
}

/// Writes `report` to a new file in the reports folder and marks it as not yet shown.
fn save_report(report: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(folder())?;
    let path = folder().join(format!("{} crash {}.txt", canvas_core::names::EDIT, crate::app_settings::now_seconds()));
    std::fs::write(&path, report)?;
    std::fs::write(unseen_marker(), path.to_string_lossy().as_bytes())?;
    Ok(path)
}

/// The report from a crash the user hasn't been told about yet, if it is still there. Reading it
/// clears the mark, so each crash is mentioned once.
fn take_unseen() -> Option<PathBuf> {
    let marker = unseen_marker();
    let path = PathBuf::from(std::fs::read_to_string(&marker).ok()?.trim());
    let _ = std::fs::remove_file(&marker);
    path.exists().then_some(path)
}

/// On launch: says Omina Edit closed unexpectedly last time, if it did, and offers to show the
/// report.
#[cfg_attr(test, allow(dead_code))]
pub fn mention_last_crash(window: &mut Window, cx: &mut Context<Workspace>) {
    let Some(report) = take_unseen() else { return };
    crate::panels::chrome::open_alert_dialog(window, cx, move |dialog, _, _| {
        let report = report.clone();
        dialog
            .title(format!("{} closed unexpectedly", canvas_core::names::EDIT))
            .description("A report of what went wrong was saved on this Mac. Sending it with a description of what you were doing helps get it fixed. It isn't sent anywhere by itself.")
            .ok_text("Show Report")
            .cancel_text("Close")
            .show_cancel(true)
            .on_ok(move |_, window, cx| {
                if let Err(error) = std::process::Command::new("open").arg("-R").arg(&report).spawn() {
                    window.push_notification(format!("Couldn't show the report: {error}").as_str(), cx);
                }
                true
            })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crash_is_saved_and_mentioned_once() {
        let report = report_text("index out of bounds", "src/viewer.rs:10", "main", "0: canvas::main");
        assert!(report.starts_with("Omina Edit ") && report.contains("Error: index out of bounds") && report.contains("Where: src/viewer.rs:10") && report.contains(env!("CARGO_PKG_VERSION")));
        let saved = save_report(&report).unwrap();
        assert_eq!(saved.parent(), Some(folder().as_path()));
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), report);
        assert_eq!(take_unseen(), Some(saved.clone()), "mentioned on the next launch");
        assert_eq!(take_unseen(), None, "only once");
        std::fs::remove_file(saved).unwrap();
    }
}
