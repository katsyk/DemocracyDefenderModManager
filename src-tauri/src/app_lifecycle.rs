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
    Relaunch { program, args: unshown_links.iter().map(OsString::from).collect() }
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
