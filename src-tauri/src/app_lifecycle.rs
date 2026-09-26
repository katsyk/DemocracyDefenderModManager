//! Decisions about closing and relaunching the app, kept as pure functions
//! so they can be tested without a window: after a data folder change the
//! app relaunches itself, and while a data folder move is copying it must
//! not be closed or force-exited.

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

/// What to start after a data folder change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relaunch {
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// The relaunch after a data folder change (move, adopt, locate, reset).
///
/// Never passes on what this process was started with (`original_args`,
/// taken only to make that explicit): a `ddmm://` link would show its
/// install prompt again, and native-messaging host arguments would start a
/// windowless relay instead of the app. The only arguments are
/// `unshown_links`: `ddmm://install` links that arrived but were never
/// shown (e.g. while the data-folder recovery screen was up), so they
/// aren't lost -- see `deep_link::DeepLinkQueue::pending_links`. The
/// environment is inherited as is, which keeps what portable/AppImage
/// detection needs (`APPIMAGE`).
///
/// The links passed on are limited to [`RELAUNCH_ARGS_BUDGET`] in total,
/// oldest first; a link that doesn't fit is left out (and a later, shorter
/// one may still fit). Too long a command line would make the relaunch
/// itself fail (Windows' 32,767-character limit, `E2BIG` on Linux).
///
/// For an AppImage the program is the `.AppImage` file itself (`$APPIMAGE`),
/// not the binary inside its temporary mount, which is gone once this
/// process exits.
pub fn relaunch_command(
    current_exe: &Path,
    appimage: Option<&OsStr>,
    original_args: &[OsString],
    unshown_links: &[String],
) -> Relaunch {
    let _ = original_args;
    let program = match appimage {
        Some(a) if !a.is_empty() => PathBuf::from(a),
        _ => current_exe.to_path_buf(),
    };
    Relaunch { program, args: links_within_budget(unshown_links, RELAUNCH_ARGS_BUDGET) }
}

/// Total size the relaunch's link arguments may take, including
/// [`RELAUNCH_ARG_OVERHEAD`] per argument. Far below any OS limit, together
/// with the program path.
pub const RELAUNCH_ARGS_BUDGET: usize = 8 * 1024;

/// What each argument costs besides its own text: a separating space and,
/// on Windows, the quotes around it.
pub const RELAUNCH_ARG_OVERHEAD: usize = 3;

/// The links, oldest first, that fit in `budget` together.
fn links_within_budget(links: &[String], budget: usize) -> Vec<OsString> {
    let mut used = 0;
    let mut args = Vec::new();
    for link in links {
        let cost = link.len() + RELAUNCH_ARG_OVERHEAD;
        if used + cost <= budget {
            used += cost;
            args.push(OsString::from(link));
        }
    }
    args
}

/// Start `relaunch`; if that fails while it carries links, try once more
/// with no arguments, so a problem with the links can never keep DDMM from
/// coming back. `spawn` is [`spawn_relaunch`] outside of tests. Returns the
/// command that was started, or the last error.
pub fn spawn_relaunch_or_plain(
    relaunch: &Relaunch,
    mut spawn: impl FnMut(&Relaunch) -> std::io::Result<()>,
) -> std::io::Result<Relaunch> {
    match spawn(relaunch) {
        Ok(()) => Ok(relaunch.clone()),
        Err(e) if relaunch.args.is_empty() => Err(e),
        Err(e) => {
            log::warn!(
                "Couldn't relaunch DDMM with {} install link(s) ({e}); relaunching without them.",
                relaunch.args.len()
            );
            let plain = Relaunch { program: relaunch.program.clone(), args: Vec::new() };
            spawn(&plain).map(|()| plain)
        }
    }
}

/// Whether a window close request may go ahead. Closing is ignored while a
/// data folder move is copying (the progress popup asks the user to wait).
pub fn close_allowed(move_running: bool) -> bool {
    !move_running
}

/// Whether an exit request (last window closed, OS request, `app.exit`
/// from the frontend's fallback) may go ahead. `code` is `None` for a
/// request the runtime made on its own (e.g. the last window closing).
/// Refused while a move is copying; the app's own relaunch happens only
/// after the move has finished, so it's never blocked by this.
pub fn exit_allowed(move_running: bool) -> bool {
    !move_running
}

/// The close watchdog's decision when its timeout fires: force the app to
/// exit only if the window is still open, the frontend never acknowledged
/// the close, **and** no data folder move is copying. A move that started
/// after the close request (or is still running) wins; the app restarts by
/// itself when the move finishes.
pub fn watchdog_should_force_exit(still_open: bool, acked: bool, move_running: bool) -> bool {
    still_open && !acked && !move_running
}

/// Start `relaunch` detached (no console, stdio closed), not waiting for it.
pub fn spawn_relaunch(relaunch: &Relaunch) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(&relaunch.program);
    cmd.args(&relaunch.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn relaunch_drops_every_original_argument() {
        let exe = Path::new("/opt/ddmm/ddmm");
        for original in [
            args(&["ddmm"]),
            args(&["ddmm", "ddmm://install?url=https%3A%2F%2Fexample.com%2Fmod.zip"]),
            args(&["ddmm", "chrome-extension://inomhciahaeeefhgdkiaabdponcfdane/", "--parent-window=0"]),
            args(&["ddmm", "/home/u/.mozilla/native-messaging-hosts/io.github.katsyk.ddmm.json", "ddmm@katsyk.github.io"]),
        ] {
            let r = relaunch_command(exe, None, &original, &[]);
            assert!(r.args.is_empty(), "{original:?} -> {r:?}");
            assert_eq!(r.program, exe);
        }
    }

    #[test]
    fn relaunch_passes_on_only_links_never_shown() {
        // Started by a link that was already shown: it isn't replayed; a
        // link that arrived on the recovery screen and was never shown is.
        let unshown = vec!["ddmm://install?url=https%3A%2F%2Fexample.org%2Fb.zip".to_string()];
        let r = relaunch_command(
            Path::new("/opt/ddmm/ddmm"),
            None,
            &args(&["ddmm", "ddmm://install?url=https%3A%2F%2Fexample.com%2Fa.zip"]),
            &unshown,
        );
        assert_eq!(r.args, args(&["ddmm://install?url=https%3A%2F%2Fexample.org%2Fb.zip"]));
    }

    /// A `ddmm://install` link `len` bytes long.
    fn link_of_len(len: usize) -> String {
        let head = "ddmm://install?url=https%3A%2F%2Fexample.com%2F";
        format!("{head}{}", "a".repeat(len - head.len()))
    }

    #[test]
    fn relaunch_links_stay_within_the_argument_budget() {
        let exe = Path::new("/opt/ddmm/ddmm");
        // The worst the queue can hold: 16 links whose 2048-character
        // targets roughly triple in length when encoded again.
        let many: Vec<String> = (0..16).map(|_| link_of_len(3 * 2048 + 40)).collect();
        let r = relaunch_command(exe, None, &[], &many);
        let total: usize = r.args.iter().map(|a| a.len() + RELAUNCH_ARG_OVERHEAD).sum();
        assert!(total <= RELAUNCH_ARGS_BUDGET, "{total}");
        assert_eq!(r.args.len(), 1, "one ~6 KB link fits, a second doesn't");

        // Oldest first; one that doesn't fit is skipped, a later short one still fits.
        let small_a = link_of_len(100);
        let huge = link_of_len(RELAUNCH_ARGS_BUDGET);
        let small_b = link_of_len(200);
        let r = relaunch_command(exe, None, &[], &[small_a.clone(), huge, small_b.clone()]);
        assert_eq!(r.args, vec![OsString::from(small_a), OsString::from(small_b)]);

        // Exactly at the budget still fits; one byte over doesn't.
        let exact = link_of_len(RELAUNCH_ARGS_BUDGET - RELAUNCH_ARG_OVERHEAD);
        assert_eq!(relaunch_command(exe, None, &[], &[exact]).args.len(), 1);
        let over = link_of_len(RELAUNCH_ARGS_BUDGET - RELAUNCH_ARG_OVERHEAD + 1);
        assert!(relaunch_command(exe, None, &[], &[over]).args.is_empty());
    }

    #[test]
    fn a_failed_relaunch_with_links_is_retried_once_without_them() {
        let with_links = Relaunch { program: PathBuf::from("/opt/ddmm/ddmm"), args: args(&["ddmm://install?url=x"]) };
        let mut attempts = Vec::new();
        let started = spawn_relaunch_or_plain(&with_links, |r| {
            attempts.push(r.clone());
            if r.args.is_empty() {
                Ok(())
            } else {
                Err(std::io::Error::other("argument list too long"))
            }
        })
        .unwrap();
        assert!(started.args.is_empty());
        assert_eq!(started.program, with_links.program);
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0], with_links);

        // Success the first time: no retry.
        let mut calls = 0;
        let started = spawn_relaunch_or_plain(&with_links, |_| {
            calls += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!((calls, started), (1, with_links.clone()));

        // With no links there's nothing to drop: a failure is final, not retried.
        let plain = Relaunch { program: with_links.program.clone(), args: Vec::new() };
        let mut calls = 0;
        assert!(spawn_relaunch_or_plain(&plain, |_| {
            calls += 1;
            Err(std::io::Error::other("no such file"))
        })
        .is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn relaunch_uses_the_appimage_file_not_its_mount() {
        let r = relaunch_command(
            Path::new("/tmp/.mount_DDMMxyz/usr/bin/ddmm"),
            Some(OsStr::new("/home/u/Apps/DDMM-x86_64.AppImage")),
            &args(&["ddmm", "ddmm://open"]),
            &[],
        );
        assert_eq!(r.program, PathBuf::from("/home/u/Apps/DDMM-x86_64.AppImage"));
        assert!(r.args.is_empty());
        // An empty APPIMAGE is ignored.
        let r = relaunch_command(Path::new("/opt/ddmm/ddmm"), Some(OsStr::new("")), &[], &[]);
        assert_eq!(r.program, PathBuf::from("/opt/ddmm/ddmm"));
    }

    #[test]
    fn closing_and_exiting_are_refused_only_while_a_move_runs() {
        assert!(close_allowed(false));
        assert!(!close_allowed(true));
        assert!(exit_allowed(false));
        assert!(!exit_allowed(true));
    }

    #[test]
    fn watchdog_never_force_exits_during_a_move() {
        // The normal case it exists for: frontend never acked, window stuck open.
        assert!(watchdog_should_force_exit(true, false, false));
        // A move running when the timeout fires always wins.
        assert!(!watchdog_should_force_exit(true, false, true));
        // Nothing to do otherwise.
        assert!(!watchdog_should_force_exit(true, true, false));
        assert!(!watchdog_should_force_exit(false, false, false));
        assert!(!watchdog_should_force_exit(false, false, true));
    }
}
