//! Launches an external video player for a stream URL.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Result, bail};

fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").unwrap_or_default();
    path.split(':')
        .chain(["/opt/homebrew/bin", "/usr/local/bin"])
        .map(|dir| Path::new(dir).join(bin))
        .find(|p| p.is_file())
}

/// Tries `$KINORA_PLAYER`, mpv, IINA, VLC, then ffplay. Returns the player name.
pub fn launch(url: &str, title: &str) -> Result<String> {
    let custom = std::env::var("KINORA_PLAYER").or_else(|_| std::env::var("STREMIO_PLAYER"));
    let mut cmd = if let Ok(player) = custom {
        let mut c = Command::new(player);
        c.arg(url);
        c
    } else if let Some(mpv) = which("mpv") {
        let mut c = Command::new(mpv);
        c.args(["--force-window=immediate", &format!("--force-media-title={title}"), url]);
        c
    } else if Path::new("/Applications/IINA.app").exists() {
        let mut c = Command::new("open");
        c.args(["-a", "IINA", url]);
        c
    } else if Path::new("/Applications/VLC.app").exists() {
        let mut c = Command::new("/Applications/VLC.app/Contents/MacOS/VLC");
        c.args(["--meta-title", title, url]);
        c
    } else if let Some(ffplay) = which("ffplay") {
        let mut c = Command::new(ffplay);
        c.args(["-autoexit", "-window_title", title, url]);
        c
    } else {
        bail!("no video player found: install mpv (brew install mpv) or VLC, or set KINORA_PLAYER");
    };
    let name = Path::new(cmd.get_program())
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut child = cmd.spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(name)
}
