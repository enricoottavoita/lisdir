//! Self-update from GitHub releases, no JSON and no extra dependencies.
//!
//! Every release publishes `SHA256SUMS.txt` next to `lisdir-setup-X.Y.Z.exe`, so that one small file tells us the
//! newest version and the installer's hash. We fetch it with the `curl.exe` that ships with Windows, download the
//! installer, check the hash, and hand it to Windows (UAC prompt). The installer replaces the app and restarts it.
use crate::service::wide;
use std::{os::windows::process::CommandExt, path::PathBuf, process::Command, ptr::null_mut};
use windows_sys::Win32::UI::Shell::ShellExecuteW;

const RELEASES: &str = "https://github.com/enricoottavoita/lisdir/releases";
const NO_WINDOW: u32 = 0x0800_0000;

/// LISDIR_UPDATE_URL points the check at another place with the same layout (a local folder, for testing).
fn base() -> String {
    std::env::var("LISDIR_UPDATE_URL").unwrap_or_else(|_| RELEASES.into())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub version: String,
    sha256: String,
}

fn version_key(v: &str) -> Option<Vec<u32>> {
    v.split('.').map(|p| p.parse().ok()).collect()
}

/// The installer line of a `sha256sum` listing: `<hash> *lisdir-setup-X.Y.Z.exe`.
fn parse(sums: &str) -> Option<Update> {
    sums.lines().find_map(|l| {
        let (hash, name) = l.split_once(' ')?;
        let version = name.trim().trim_start_matches('*').strip_prefix("lisdir-setup-")?.strip_suffix(".exe")?;
        version_key(version)?;
        (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| Update { version: version.into(), sha256: hash.to_lowercase() })
    })
}

fn curl(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new("curl.exe").args(["-sSfL", "--max-time", "600"]).args(args).creation_flags(NO_WINDOW).output().map_err(|e| e.to_string())?;
    if out.status.success() { Ok(out.stdout) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

fn check_at(base: &str, current: &str) -> Result<Option<Update>, String> {
    let sums = curl(&[&format!("{base}/latest/download/SHA256SUMS.txt")])?;
    let newest = parse(&String::from_utf8_lossy(&sums)).ok_or("no installer in the release")?;
    Ok((version_key(&newest.version) > version_key(current)).then_some(newest))
}

/// Is a newer release published?
pub fn check() -> Result<Option<Update>, String> {
    check_at(&base(), env!("CARGO_PKG_VERSION"))
}

fn sha256_of(file: &PathBuf) -> Result<String, String> {
    let out = Command::new("certutil").args(["-hashfile"]).arg(file).arg("SHA256").creation_flags(NO_WINDOW).output().map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout);
    // the hash is the second line; older certutil versions put spaces between bytes
    text.lines().nth(1).map(|l| l.split_whitespace().collect::<String>().to_lowercase()).ok_or_else(|| "cannot hash the download".into())
}

/// Download the installer into `dir` and verify it against the published hash.
fn fetch_at(base: &str, u: &Update, dir: &PathBuf) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let file = dir.join(format!("lisdir-setup-{}.exe", u.version));
    let _ = std::fs::remove_file(&file);
    curl(&["-o", &file.to_string_lossy(), &format!("{base}/download/v{0}/lisdir-setup-{0}.exe", u.version)])?;
    if sha256_of(&file)? != u.sha256 {
        let _ = std::fs::remove_file(&file);
        return Err("the download does not match its checksum".into());
    }
    Ok(file)
}

/// Download, verify and start the installer (Windows asks for elevation). Ok means it started: quit so it can replace us.
pub fn install(u: &Update) -> Result<(), String> {
    let file = fetch_at(&base(), u, &std::env::temp_dir().join("lisdir-update"))?;
    // silent wizard; /TASKS="" leaves the autostart choice alone; /START=1 reopens lisdir afterwards
    let run = unsafe {
        ShellExecuteW(null_mut(), wide("open").as_ptr(), wide(&file.to_string_lossy()).as_ptr(), wide(r#"/SILENT /TASKS="" /START=1"#).as_ptr(), std::ptr::null(), 1)
    };
    if run as usize > 32 { Ok(()) } else { Err("the installer was not started".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "681948c00eb587f569c5a2c82199f7c077ce3c5404fc8bf569916aa272f1a772";

    #[test]
    fn reads_the_installer_line() {
        let sums = format!("{} *lisdir-0.13.0-portable.exe\n{HASH} *lisdir-setup-0.13.0.exe\n", "9".repeat(64));
        assert_eq!(parse(&sums), Some(Update { version: "0.13.0".into(), sha256: HASH.into() }));
        assert_eq!(parse("abc *lisdir-setup-0.13.0.exe"), None); // not a sha256
        assert_eq!(parse(&format!("{HASH} *lisdir-setup-x.exe")), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn compares_versions_numerically() {
        assert!(version_key("0.13.10") > version_key("0.13.9"));
        assert!(version_key("0.14.0") > version_key("0.13.99"));
        assert!(version_key("0.13.1") > version_key("0.13.0"));
    }

    /// Fake release folder: check, download and hash verification end to end through curl and certutil.
    #[test]
    fn checks_and_downloads_from_a_folder() {
        let root = std::env::temp_dir().join(format!("lisdir-update-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (latest, dl) = (root.join("latest/download"), root.join("download/v9.9.9"));
        std::fs::create_dir_all(&latest).unwrap();
        std::fs::create_dir_all(&dl).unwrap();
        std::fs::write(dl.join("lisdir-setup-9.9.9.exe"), b"installer bytes").unwrap();
        let hash = sha256_of(&dl.join("lisdir-setup-9.9.9.exe")).unwrap();
        std::fs::write(latest.join("SHA256SUMS.txt"), format!("{hash} *lisdir-setup-9.9.9.exe\n")).unwrap();
        let base = format!("file:///{}", root.to_string_lossy().replace('\\', "/"));

        assert_eq!(check_at(&base, "9.9.9"), Ok(None)); // already current
        let u = check_at(&base, "0.13.1").unwrap().expect("newer release");
        assert_eq!(u.version, "9.9.9");
        let got = fetch_at(&base, &u, &root.join("out")).unwrap();
        assert_eq!(std::fs::read(got).unwrap(), b"installer bytes");

        let bad = Update { sha256: "0".repeat(64), ..u };
        assert!(fetch_at(&base, &bad, &root.join("out2")).unwrap_err().contains("checksum"));
        assert!(!root.join("out2/lisdir-setup-9.9.9.exe").exists()); // a bad download is deleted
        let _ = std::fs::remove_dir_all(&root);
    }
}
