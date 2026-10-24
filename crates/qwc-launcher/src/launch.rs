//! Starting the game: `qwc-game` beside the launcher, run on the player's profile from the folder that holds the
//! remaster's shipped files (`remastered/fonts.txt`, the stock screen layouts and looks), its output kept in the
//! library's `logs/game.log`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// The game's executable: beside the launcher's.
pub fn game_exe() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let exe = dir.join(if cfg!(windows) { "qwc-game.exe" } else { "qwc-game" });
    exe.is_file().then_some(exe)
}

/// The folder holding the remaster's shipped files: the launcher's folder or the nearest above it with
/// `remastered/fonts.txt` (a build in the repo's `target/` finds the repo's root), else the launcher's folder.
pub fn resources_root() -> PathBuf {
    let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)).unwrap_or_default();
    exe_dir.ancestors().find(|d| d.join("remastered/fonts.txt").is_file()).map_or(exe_dir.clone(), Path::to_path_buf)
}

/// QWC Studio's executable, beside the launcher's (none when it is not built or shipped there).
pub fn studio_exe() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let exe = dir.join(if cfg!(windows) { "qwc-studio.exe" } else { "qwc-studio" });
    exe.is_file().then_some(exe)
}

/// Open QWC Studio on its Packs mode, from the folder of the shipped files.
pub fn studio() -> Result<(), String> {
    let exe = studio_exe().ok_or("QWC Studio is not beside the launcher")?;
    let mut c = Command::new(&exe);
    no_console(&mut c);
    c.arg("--packs").current_dir(resources_root()).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// On Windows, start a console program (the game, a debug build's Studio) without a console window of its own:
/// the launcher is a window, so the system would open one. Its output goes where the caller sends it.
fn no_console(c: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    let _ = c;
}

/// Show `dir` in the system's file manager.
pub fn open_folder(dir: &Path) {
    let opener = if cfg!(windows) { "explorer" } else { "xdg-open" };
    let _ = Command::new(opener).arg(dir).spawn();
}

/// Start the game's front end from the boot on `profile`, its output to `log`.
pub fn play(profile: &Path, log: &Path) -> Result<Child, String> {
    let exe = game_exe().ok_or("qwc-game is not beside the launcher")?;
    if let Some(d) = log.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let out = std::fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut c = Command::new(&exe);
    no_console(&mut c);
    c.arg("--profile")
        .arg(profile)
        .args(["frontend", "boot"])
        .current_dir(resources_root())
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .spawn()
        .map_err(|e| format!("{}: {e}", exe.display()))
}

/// Start an online match on `profile` (`qwc-game online host|join ...`, `args` after `online`): its status lines
/// (`@online ...`, standard output) to the answer's pipe, its standard input from the answer's (the host's `punch
/// ADDRESS` lines), the rest of its output to `log`.
pub fn online(profile: &Path, log: &Path, args: &[String]) -> Result<Child, String> {
    let exe = game_exe().ok_or("qwc-game is not beside the launcher")?;
    if let Some(d) = log.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let err = std::fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let mut c = Command::new(&exe);
    no_console(&mut c);
    c.arg("--profile")
        .arg(profile)
        .arg("online")
        .args(args)
        // a host waiting for a joiner leaves when the launcher does (its standard input closes)
        .env("QWC_FROM_LAUNCHER", "1")
        .current_dir(resources_root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(err)
        .spawn()
        .map_err(|e| format!("{}: {e}", exe.display()))
}

#[cfg(test)]
mod tests {
    /// In the repo (with its shipped fonts checked out), a build under `target/` finds the repo's root.
    #[test]
    fn finds_the_shipped_files_from_a_build() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        if !repo.join("remastered/fonts.txt").is_file() {
            return;
        }
        assert!(super::resources_root().join("remastered/fonts.txt").is_file());
    }
}
