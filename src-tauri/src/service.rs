//! Privilege split: an elevated, windowless helper (`lisdir --helper`) owns the
//! NTFS indexes (raw volume access needs admin) and answers searches over a named pipe; the UI runs as the
//! normal user and is only a client. The pipe is ACL'd to the current user and SYSTEM.
//!
//! Wire format, one request per line: `S\t<query>\n`. Reply: `H\t<score>\t<is_dir>\t<name>\t<path>\t<size>\n`*
//! then `E\t<busy 0|1>\t<item count>\n`. An empty query returns only the `E` line (a status probe). `Q\n` makes
//! the helper exit.
use crate::{engine::{self, wide, NO_WINDOW}, query::{Clock, Query}};
use serde::Serialize;
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Write},
    mem::size_of,
    os::windows::{io::FromRawHandle, process::CommandExt},
    ptr::null_mut,
    sync::{atomic::{AtomicUsize, Ordering::SeqCst}, Arc, RwLock},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, LocalFree, ERROR_PIPE_CONNECTED, INVALID_HANDLE_VALUE},
    Security::{
        Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW},
        GetTokenInformation, TokenUser, SECURITY_ATTRIBUTES, TOKEN_USER,
    },
    Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX},
    System::{
        Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT},
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
    UI::{Shell::ShellExecuteW, WindowsAndMessaging::{MessageBoxW, MB_ICONERROR}},
};

const PIPE: &str = r"\\.\pipe\lisdir";
const TASK: &str = "lisdir-helper";

pub type Shared = Arc<RwLock<engine::Volume>>;
pub type Volumes = Arc<RwLock<Vec<Shared>>>;

#[derive(Serialize)]
pub struct Hit {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    /// cache key for the file-type icon, see `icons::key`
    pub icon: String,
    /// a launchable app (Start menu / Store) rather than a file: `path` is then a `shell:AppsFolder\...` target
    pub app: bool,
    /// char positions of `name` to highlight (filled in by the UI process, not sent over the pipe)
    pub hl: Vec<usize>,
    /// bytes (a folder: the files below it); 0 for apps and when unknown (the UI shows nothing then)
    pub size: u64,
    /// pinned by the user: listed first (filled in by the UI process, not sent over the pipe)
    pub pinned: bool,
    /// name of the other PC this result lives on; empty for this PC
    pub peer: String,
    #[serde(skip)]
    pub score: f32,
}

impl Hit {
    pub fn new(name: String, path: String, is_dir: bool, score: f32) -> Self {
        Hit { icon: crate::icons::key(&name, &path, is_dir), app: false, hl: vec![], size: 0, pinned: false, peer: String::new(), name, path, is_dir, score }
    }

    pub fn with_size(mut self, size: u64) -> Self {
        self.size = size;
        self
    }

    pub fn app(name: &str, id: &str, score: f32) -> Self {
        Hit { icon: format!("app:{id}"), app: true, hl: vec![], name: name.into(), path: format!(r"shell:AppsFolder\{id}"), is_dir: false, size: 0, pinned: false, peer: String::new(), score }
    }
}

pub struct Reply {
    pub hits: Vec<Hit>,
    pub busy: bool,
    pub count: usize,
}

/// Best 20 matches across all volumes, highest score first.
pub fn search_volumes(vols: &Volumes, q: &Query) -> Vec<Hit> {
    let mut hits = vec![];
    for vol in vols.read().unwrap().iter() {
        let v = vol.read().unwrap();
        hits.extend(engine::search(&v.idx, q, 20).into_iter().map(|(score, f)| {
            Hit::new(v.idx[&f].name.clone(), engine::full_path(&v.idx, f, &v.root), v.idx[&f].is_dir, score).with_size(v.idx[&f].size)
        }));
    }
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    hits.truncate(20);
    hits
}

pub fn count(vols: &Volumes) -> usize {
    vols.read().unwrap().iter().map(|v| v.read().unwrap().idx.len()).sum()
}

// ---------------------------------------------------------------- helper (elevated)

/// Keep a volume current by polling the USN journal. Changes are appended to a small log; the whole cache is
/// rewritten only when the log gets big, a log write fails, or the journal had to be rescanned.
fn tail(vol: Shared) {
    let (letter, root) = {
        let v = vol.read().unwrap();
        (v.root.chars().next().unwrap_or('C'), v.root.clone())
    };
    let (mut last_save, mut unlogged) = (Instant::now(), false);
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let polled = engine::read_changes(&vol.read().unwrap());
        match polled {
            Ok((c, n)) => {
                unlogged |= !engine::append_log(&root, &c, n);
                engine::apply(&mut vol.write().unwrap(), c, n);
            }
            // journal wrapped or was recreated: our position is gone, start over
            Err(_) => match engine::scan_volume(letter) {
                Ok(fresh) => {
                    *vol.write().unwrap() = fresh;
                    unlogged = true;
                }
                Err(_) => return,
            },
        }
        // a snapshot also every few hours, so a long-running log never has to be replayed in full
        let stale = last_save.elapsed().as_secs() >= 6 * 3600;
        if (unlogged || engine::log_is_big(&root) || stale) && vol.read().unwrap().dirty {
            if engine::save_shared(&vol).is_ok() {
                (last_save, unlogged) = (Instant::now(), false);
            }
        }
    }
}

/// Load every local NTFS volume; each becomes searchable as soon as it is ready.
fn load_ntfs(vols: Volumes, busy: Arc<AtomicUsize>) {
    busy.fetch_add(1, SeqCst);
    let settings = crate::settings::Settings::load();
    crate::rules::set(&settings.no_index, &settings.exclude);
    for l in 'A'..='Z' {
        if engine::is_network_drive(l) || settings.disk_is_off(l) {
            continue; // network drives are the UI's job: it has the user's credentials and drive mappings
        }
        if let Ok(mut v) = engine::open(l) {
            let _ = engine::save(&mut v);
            let live = engine::live(&v);
            let v: Shared = Arc::new(RwLock::new(v));
            vols.write().unwrap().push(v.clone());
            if live {
                std::thread::spawn(move || tail(v));
            }
        }
    }
    busy.fetch_sub(1, SeqCst);
}

/// Pipe security: full access for the current user and SYSTEM only. None => refuse to serve.
fn pipe_security() -> Option<SECURITY_ATTRIBUTES> {
    unsafe {
        let mut tok = null_mut();
        if OpenProcessToken(GetCurrentProcess(), 8 /* TOKEN_QUERY */, &mut tok) == 0 {
            return None;
        }
        let (mut buf, mut n) = ([0u8; 256], 0u32);
        let ok = GetTokenInformation(tok, TokenUser, buf.as_mut_ptr().cast(), buf.len() as u32, &mut n);
        CloseHandle(tok);
        if ok == 0 {
            return None;
        }
        let mut sid_w = null_mut();
        if ConvertSidToStringSidW((*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid, &mut sid_w) == 0 {
            return None;
        }
        let len = (0..).take_while(|&i| *sid_w.add(i) != 0).count();
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(sid_w, len));
        LocalFree(sid_w.cast());
        let mut sd = null_mut();
        let sddl = wide(&format!("D:(A;;GA;;;{sid})(A;;GA;;;SY)"));
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), 1, &mut sd, null_mut()) == 0 {
            return None;
        }
        Some(SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd, bInheritHandle: 0 })
    }
}

fn answer(q: &str, vols: &Volumes, busy: &AtomicUsize) -> String {
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    let mut out = String::new();
    for h in search_volumes(vols, &Query::parse(q, &Clock::system())) {
        out += &format!("H\t{}\t{}\t{}\t{}\t{}\n", h.score, h.is_dir as u8, clean(&h.name), clean(&h.path), h.size);
    }
    out + &format!("E\t{}\t{}\n", (busy.load(SeqCst) > 0) as u8, count(vols))
}

fn handle(f: File, vols: Volumes, busy: Arc<AtomicUsize>) {
    let (mut r, mut w, mut line) = (BufReader::new(&f), &f, String::new());
    while r.read_line(&mut line).unwrap_or(0) > 0 {
        if line.trim_end() == "Q" {
            std::process::exit(0); // a portable update replaced our exe, see `stop_helper`
        }
        let q = line.trim_end().strip_prefix("S\t").unwrap_or("").to_string();
        line.clear();
        if w.write_all(answer(&q, &vols, &busy).as_bytes()).is_err() {
            break;
        }
    }
}

/// `lisdir --helper`: load the indexes and serve the pipe forever.
pub fn run_helper() {
    let (vols, busy): (Volumes, _) = (Default::default(), Arc::new(AtomicUsize::new(0)));
    let Some(sa) = pipe_security() else { return };
    let name = wide(PIPE);
    let mut first = true;
    // Create the first pipe instance before loading so the UI can connect (and see "indexing") immediately.
    let mut instance = || unsafe {
        let flags = PIPE_ACCESS_DUPLEX | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
        let h = CreateNamedPipeW(name.as_ptr(), flags, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES, 1 << 16, 1 << 16, 0, &sa);
        first = false;
        h
    };
    let mut h = instance();
    if h == INVALID_HANDLE_VALUE {
        return; // another helper already owns the pipe
    }
    let (v2, b2) = (vols.clone(), busy.clone());
    std::thread::spawn(move || load_ntfs(v2, b2));
    loop {
        let connected = unsafe { ConnectNamedPipe(h, null_mut()) != 0 || GetLastError() == ERROR_PIPE_CONNECTED };
        let file = unsafe { File::from_raw_handle(h) }; // closes the instance when dropped
        if connected {
            let (v, b) = (vols.clone(), busy.clone());
            std::thread::spawn(move || handle(file, v, b));
        }
        h = instance();
        while h == INVALID_HANDLE_VALUE {
            std::thread::sleep(Duration::from_secs(1));
            h = instance();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_filtered_queries_in_the_wire_format() {
        let vols: Volumes = Arc::new(RwLock::new(vec![Arc::new(RwLock::new(engine::sample_volume()))]));
        let busy = AtomicUsize::new(0);
        let lines = |q: &str| answer(q, &vols, &busy).lines().map(String::from).collect::<Vec<_>>();

        // filters travel as plain text and are parsed on the helper's side
        let r = lines("report ext:pdf -draft");
        assert_eq!(r.len(), 2);
        let f: Vec<&str> = r[0].split('\t').collect();
        assert_eq!((f[0], f[2], f[3], f[4]), ("H", "0", "report 2024.pdf", r"C:\Docs\report 2024.pdf"));
        assert!(f[1].parse::<f32>().is_ok());
        assert_eq!(f[5], "2048"); // the size travels as the last field
        assert_eq!(r[1], "E\t0\t3"); // not busy, three indexed items

        assert_eq!(lines(r"docs\ report").len(), 3); // both reports are in a folder containing "docs"
        assert_eq!(lines(""), ["E\t0\t3"]); // an empty query is a status probe
        assert_eq!(lines("app: report"), ["E\t0\t3"]); // apps only: no file hits
    }
}

// ---------------------------------------------------------------- client (UI, unelevated)

/// Ask the helper. None when it is not running.
pub fn query(q: &str) -> Option<Reply> {
    let f = OpenOptions::new().read(true).write(true).open(PIPE).ok()?;
    writeln!(&f, "S\t{}", q.replace(['\t', '\n', '\r'], " ")).ok()?;
    let mut hits = vec![];
    for line in BufReader::new(&f).lines() {
        let line = line.ok()?;
        let mut p = line.splitn(6, '\t');
        match p.next()? {
            "H" => {
                let (score, is_dir, name, path) = (p.next()?, p.next()?, p.next()?, p.next()?);
                let size = p.next().and_then(|s| s.parse().ok()).unwrap_or(0); // absent from an older helper
                hits.push(Hit::new(name.into(), path.into(), is_dir == "1", score.parse().ok()?).with_size(size));
            }
            "E" => return Some(Reply { hits, busy: p.next()? == "1", count: p.next()?.parse().ok()? }),
            _ => return None,
        }
    }
    None
}

fn quiet(cmd: &str, args: &[&str]) -> bool {
    std::process::Command::new(cmd).args(args).creation_flags(NO_WINDOW).status().is_ok_and(|s| s.success())
}

/// Start the (already installed) elevated helper task. Needs no UAC prompt: running your own task is allowed.
pub fn start_helper() -> bool {
    quiet("schtasks", &["/Run", "/TN", TASK])
}

/// `stop_helper`, then wait (up to 5 s) until the pipe is gone, so a new helper can take its place.
pub fn stop_helper_and_wait() {
    stop_helper();
    for _ in 0..25 {
        if OpenOptions::new().write(true).open(PIPE).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Ask the helper to quit (`Q`), so the task can start it again from a replaced exe. A helper older than 0.14.3
/// ignores `Q`; `/End` is the fallback, which may be refused unelevated: then the old helper serves until sign-out.
pub fn stop_helper() {
    if let Ok(f) = OpenOptions::new().write(true).open(PIPE) {
        let _ = writeln!(&f, "Q");
    }
    quiet("schtasks", &["/End", "/TN", TASK]);
}

/// Ask Windows for elevation and re-run ourselves as `--install`.
pub fn request_install() {
    if let Ok(exe) = std::env::current_exe() {
        unsafe { ShellExecuteW(null_mut(), wide("runas").as_ptr(), wide(&exe.to_string_lossy()).as_ptr(), wide("--install").as_ptr(), std::ptr::null(), 0) };
    }
}

/// `lisdir --install` (elevated): register a logon task that runs the helper at highest privilege, and start it.
pub fn install() {
    let ok = (|| -> std::io::Result<bool> {
        let exe = std::env::current_exe()?;
        let user = format!("{}\\{}", std::env::var("USERDOMAIN").unwrap_or_default(), std::env::var("USERNAME").unwrap_or_default());
        // Settings matter: the defaults of a command-line task stop it on battery and after 3 days.
        let xml = format!(r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="A"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="A"><Exec><Command>{}</Command><Arguments>--helper</Arguments></Exec></Actions>
</Task>"#, exe.display());
        let path = engine::data_dir().join("helper-task.xml");
        std::fs::create_dir_all(engine::data_dir())?;
        let bytes: Vec<u8> = std::iter::once(0xFEFF).chain(xml.encode_utf16()).flat_map(u16::to_le_bytes).collect(); // schtasks wants UTF-16 with BOM
        std::fs::write(&path, bytes)?;
        Ok(quiet("schtasks", &["/Create", "/TN", TASK, "/XML", &path.to_string_lossy(), "/F"]) && start_helper())
    })();
    if !ok.unwrap_or(false) {
        unsafe { MessageBoxW(null_mut(), wide(crate::t("Could not install the lisdir index service.", "Impossibile installare il servizio di indicizzazione di lisdir.")).as_ptr(), wide("lisdir").as_ptr(), MB_ICONERROR) };
    }
}
