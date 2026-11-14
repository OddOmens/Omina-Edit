//! Checking for a newer Omina Edit, as Omina Motion does. A release build names its update
//! feed with the `CANVAS_UPDATE_URL` environment variable when it is compiled
//! (`CANVAS_UPDATE_URL=https://… cargo build --release`): an https address of a small JSON file
//! describing the latest release:
//!
//! ```json
//! { "version": "0.2.0", "url": "https://…/Omina-Edit-0.2.0.dmg", "notes": "What's new." }
//! ```
//!
//! Omina Edit checks it once a day when it starts, and from Omina Edit ▸ Check for Updates. It
//! never downloads or installs anything itself: a newer version is offered as a link to open. A
//! build without a feed (a developer's own build, say) has the menu item turned off.

use gpui_kit::component::WindowExt as _;
use gpui_kit::{Context, Window};
use serde::Deserialize;

use crate::workspace::Workspace;

/// The update feed this build checks, if it was given one when it was compiled. There is no
/// address built in: the owner of the release names it.
pub const FEED: Option<&str> = option_env!("CANVAS_UPDATE_URL");

/// How often the automatic check runs: once a day.
const CHECK_EVERY_SECONDS: u64 = 24 * 60 * 60;

/// The feed is a few hundred bytes; anything much larger is not it.
const MAX_FEED_BYTES: u64 = 64 * 1024;

/// How Omina Edit names itself to the server it asks. It says nothing about the person or their
/// computer.
const USER_AGENT: &str = concat!("Canvas/", env!("CARGO_PKG_VERSION"), " (image editor)");

/// The latest release, as the feed describes it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Release {
    pub version: String,
    /// Where it can be downloaded (https only).
    pub url: String,
    #[serde(default)]
    pub notes: String,
}

/// Whether version `candidate` (such as "0.10.2") is newer than `current`, comparing each
/// dot-separated number in turn. Anything after a hyphen ("1.0.0-beta") is ignored; a part that
/// isn't a number counts as 0.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parts = |version: &str| -> Vec<u64> {
        let core = version.trim().trim_start_matches('v').split('-').next().unwrap_or_default();
        let mut numbers: Vec<u64> = core.split('.').map(|part| part.parse().unwrap_or(0)).collect();
        // "1.2" and "1.2.0" are the same version.
        while numbers.last() == Some(&0) {
            numbers.pop();
        }
        numbers
    };
    parts(candidate) > parts(current)
}

/// Reads the feed's text, refusing a release whose link isn't https.
pub fn parse_feed(text: &str) -> Result<Release, String> {
    let release: Release = serde_json::from_str(text).map_err(|error| format!("the update information couldn't be read ({error})"))?;
    if !release.url.starts_with("https://") {
        return Err("the update information has an unsafe download link".into());
    }
    Ok(release)
}

/// Fetches the feed with the system's `curl`, over https only, giving up after 15 seconds so a
/// slow network never leaves a check hanging.
fn fetch_feed(url: &str) -> Result<String, String> {
    let output = std::process::Command::new("curl")
        .args(["-q", "-sSfL", "-A", USER_AGENT, "--proto", "=https", "--proto-redir", "=https", "--connect-timeout", "10", "--max-time", "15", "--max-filesize"])
        .arg(MAX_FEED_BYTES.to_string())
        .arg(url)
        .output()
        .map_err(|error| format!("could not start the request: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().trim_start_matches("curl: ").to_owned());
    }
    String::from_utf8(output.stdout).map_err(|_| "the reply was not text".to_owned())
}

/// Checks the feed in the background. `asked` is true for Check for Updates, which always says
/// what it found; the daily check at launch only speaks up when there is a newer version, and
/// skips itself if it ran in the last day.
pub fn check(workspace: &mut Workspace, asked: bool, window: &mut Window, cx: &mut Context<Workspace>) {
    let Some(feed) = FEED else {
        if asked {
            window.push_notification(format!("This copy of {} doesn't have an update address, so it can't check for updates.", canvas_core::names::EDIT).as_str(), cx);
        }
        return;
    };
    let now = crate::app_settings::now_seconds();
    if !asked && now.saturating_sub(crate::app_settings::load().last_update_check) < CHECK_EVERY_SECONDS {
        return;
    }
    if workspace.ui.checking_for_updates {
        return;
    }
    workspace.ui.checking_for_updates = true;
    let fetched = cx.background_executor().spawn(async move { fetch_feed(feed).and_then(|text| parse_feed(&text)) });
    cx.spawn_in(window, async move |this, cx| {
        let result = fetched.await;
        this.update_in(cx, |this, window, cx| {
            this.ui.checking_for_updates = false;
            // Only a check that reached the feed counts, so a missed one is tried next launch.
            if result.is_ok() {
                crate::app_settings::update(|settings| settings.last_update_check = now);
            }
            show_result(result, asked, window, cx);
        })
        .ok();
    })
    .detach();
}

/// Offers a newer version, or (when the check was asked for) says Omina Edit is up to date or
/// why the check failed.
fn show_result(result: Result<Release, String>, asked: bool, window: &mut Window, cx: &mut Context<Workspace>) {
    let (current, edit) = (env!("CARGO_PKG_VERSION"), canvas_core::names::EDIT);
    match result {
        Ok(release) if is_newer(&release.version, current) => {
            let title = format!("{edit} {} is available", release.version);
            let notes = if release.notes.trim().is_empty() { String::new() } else { format!("\n\n{}", release.notes.trim()) };
            let description = format!("You have version {current}. Download the new version, then replace {edit} in your Applications folder with it. Your projects and settings are kept.{notes}");
            let url = release.url.clone();
            crate::panels::chrome::open_alert_dialog(window, cx, move |dialog, _, _| {
                let url = url.clone();
                dialog.title(title.clone()).description(description.clone()).ok_text("Download").cancel_text("Later").show_cancel(true).on_ok(move |_, _, cx| {
                    cx.open_url(&url);
                    true
                })
            });
        }
        Ok(_) if asked => window.push_notification(format!("{edit} {current} is the latest version.").as_str(), cx),
        Err(error) if asked => window.push_notification(format!("Couldn't check for updates: {error}").as_str(), cx),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_number_by_number() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.9"), "numbers, not text");
        assert!(is_newer("1.0", "0.99.99"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1", "0.1.0"), "a missing part is zero");
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("v1.2.1", "1.2.0"), "a leading v is allowed");
        assert!(!is_newer("1.2.0-beta", "1.2.0"));
    }

    #[test]
    fn the_feed_must_link_to_https() {
        let release = parse_feed(r#"{"version": "0.2.0", "url": "https://example.com/Edit.dmg", "notes": "Faster exports."}"#).unwrap();
        assert_eq!((release.version.as_str(), release.notes.as_str()), ("0.2.0", "Faster exports."));
        assert!(parse_feed(r#"{"version": "0.2.0", "url": "http://example.com/Edit.dmg"}"#).is_err());
        assert!(parse_feed(r#"{"version": "0.2.0", "url": "file:///Applications/Calculator.app"}"#).is_err());
        assert!(parse_feed("<html>Not found</html>").is_err());
        assert_eq!(parse_feed(r#"{"version": "0.3.0", "url": "https://example.com/a.dmg"}"#).unwrap().notes, "", "notes are optional");
    }
}
