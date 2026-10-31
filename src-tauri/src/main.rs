#![cfg_attr(not(any(debug_assertions, test)), windows_subsystem = "windows")]
mod actions;
mod apps;
mod bar;
mod engine;
mod gfx;
mod icons;
mod mft;
mod peer;
mod prefs;
mod query;
mod rules;
mod service;
mod settings;
mod tray;
mod typesearch;
mod update;
mod usage;

use query::{Clock, Query};
use service::{Hit, Shared, Volumes};
use std::{
    os::windows::process::CommandExt,
    sync::{atomic::{AtomicUsize, Ordering}, Arc, OnceLock, RwLock},
};
use windows_sys::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, TranslateMessage};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// This process runs unelevated and only indexes network folders (it has the user's credentials and
/// drive mappings). Local NTFS volumes are searched through the elevated helper, see `service`.
#[derive(Default)]
struct App {
    network: Volumes,
    /// Start menu and Store apps, loaded in the background at startup
    apps: Arc<RwLock<Vec<apps::App>>>,
    /// when the app list was last requested, so the Start menu is re-read at most once a minute
    apps_at: std::sync::Mutex<Option<std::time::Instant>>,
    /// what you open: boosts frequent results and feeds the recent list
    usage: std::sync::Mutex<usage::Usage>,
    /// pinned items, listed first; kept in the same file format as `usage` (newest pin first)
    pins: std::sync::Mutex<usage::Usage>,
    /// number of network walks in progress; non-zero shows "Indexing…"
    busy: Arc<AtomicUsize>,
    /// a newer release found by the update check, waiting for the user to accept it
    update: std::sync::Mutex<Option<update::Update>>,
    settings: RwLock<settings::Settings>,
    /// the settings the index was built with; a difference in disks, folders or rules needs a restart to apply
    launched: settings::Settings,
}

static APP: OnceLock<App> = OnceLock::new();

fn app() -> &'static App {
    APP.get().expect("set at the start of main")
}

/// Keep a network volume current by periodically re-walking it. It is searchable from the cache
/// (or empty) right away; a first walk of a big share can take minutes.
fn refresh(vol: Shared, mut stale: bool, busy: Arc<AtomicUsize>) {
    let root = vol.read().unwrap().root.clone();
    let mut wait = engine::WALK_INTERVAL;
    loop {
        if stale {
            busy.fetch_add(1, Ordering::SeqCst);
            // with nothing indexed yet, show entries as they are found instead of after the whole walk
            let streamed = vol.read().unwrap().idx.is_empty();
            let w = engine::walk(&root, streamed.then_some(&*vol));
            if w.reliable() {
                {
                    let mut v = vol.write().unwrap();
                    if !streamed {
                        v.idx = w.idx;
                    }
                    v.dirty = true;
                }
                let _ = engine::save_shared(&vol);
                wait = engine::WALK_INTERVAL;
            } else {
                // share offline or drive unplugged: keep the old results (and cache) and retry soon
                wait = engine::RETRY_INTERVAL;
            }
            busy.fetch_sub(1, Ordering::SeqCst);
        }
        std::thread::sleep(wait);
        stale = true;
    }
}

/// Re-read the Start menu in the background (a new install shows up the next time the bar opens). Throttled
/// because it starts PowerShell.
fn reload_apps(app: &App) {
    let mut at = app.apps_at.lock().unwrap();
    if at.is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(60)) {
        return;
    }
    *at = Some(std::time::Instant::now());
    let apps = app.apps.clone();
    std::thread::spawn(move || {
        let list = apps::load();
        if !list.is_empty() {
            *apps.write().unwrap() = list; // a failed read keeps the old list
        }
    });
}

/// Start the network walks and make sure the elevated helper is running.
fn start(app: &App) {
    *app.usage.lock().unwrap() = usage::Usage::load(engine::data_dir().join("usage.txt"));
    *app.pins.lock().unwrap() = usage::Usage::load(engine::data_dir().join("pins.txt"));
    std::thread::spawn(|| {
        if service::query("").is_none() {
            service::start_helper(); // no-op (fails quietly) until the service has been installed
        }
    });
    reload_apps(app);
    let (folders, disks_off) = {
        let s = app.settings.read().unwrap();
        (s.folders.clone(), s.disks_off.clone())
    };
    for root in engine::walk_roots(&folders, &disks_off) {
        let (v, fresh) = engine::open_walk(&root);
        let v: Shared = Arc::new(RwLock::new(v));
        app.network.write().unwrap().push(v.clone());
        let busy = app.busy.clone();
        std::thread::spawn(move || refresh(v, !fresh, busy));
    }
}

/// The newest `n` items of a list, as results.
fn listed(u: &usage::Usage, n: usize) -> Vec<Hit> {
    u.recent(n).into_iter().map(|(path, i)| match path.strip_prefix(r"shell:AppsFolder\") {
        Some(id) if i.app => Hit::app(&i.name, id, 0.0),
        _ => Hit::new(i.name.clone(), path.to_string(), i.is_dir, 0.0),
    }).collect()
}

/// Before you type anything: your pinned items, then the items you opened most recently.
fn recents(app: &App) -> Vec<Hit> {
    let mut hits = listed(&app.pins.lock().unwrap(), 50);
    hits.iter_mut().for_each(|h| h.pinned = true);
    let recent: Vec<Hit> = listed(&app.usage.lock().unwrap(), 10 + hits.len()).into_iter().filter(|h| !hits.iter().any(|p| p.path == h.path)).take(10).collect();
    hits.extend(recent);
    hits
}

/// The files this PC finds for a search text (also what the other PCs get when they ask).
fn files(app: &App, q: &str, query: &Query) -> Vec<Hit> {
    let mut hits = vec![];
    if query.searches_files() {
        hits = service::search_volumes(&app.network, query);
        hits.extend(service::query(q).map(|r| r.hits).unwrap_or_default());
        // apps are ranked together with files; their Start menu shortcuts would only duplicate them
        hits.retain(|h| !(h.path.contains(r"\Start Menu\Programs\") && h.path.to_lowercase().ends_with(".lnk")));
    }
    hits
}

/// What the other PCs see of this one: its best 20 files for the search text.
fn shared_files(app: &App, q: &str) -> Vec<Hit> {
    let query = Query::parse(q, &Clock::system());
    if query.is_empty() {
        return vec![];
    }
    let mut hits = files(app, q, &query);
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    hits.truncate(20);
    hits
}

/// The same search on the other PCs, asked after the local answer is on screen. Empty when the feature is off.
async fn remote_search(q: String) -> Vec<Hit> {
    if q.trim().is_empty() {
        return vec![];
    }
    let query = Query::parse(&q, &Clock::system());
    let mut hits = peer::search(q).await;
    hits.iter_mut().for_each(|h| h.hl = engine::highlight(&query, &h.name));
    hits
}

fn search(q: &str) -> Vec<Hit> {
    let app = app();
    let query = Query::parse(q, &Clock::system());
    if query.is_empty() {
        return recents(app);
    }
    let mut hits = files(app, q, &query);
    if query.wants_apps() {
        // an app found by name sits with the folders, ahead of the files
        let tier = if query.terms.is_empty() { 0.0 } else { engine::FIRST };
        hits.extend(apps::search(&app.apps.read().unwrap(), &query, 20).into_iter().map(|(score, a)| Hit::app(&a.name, &a.id, score + tier)));
    }
    if !query.terms.is_empty() {
        // habits only nudge the order: how well the name matches still comes first
        let usage = app.usage.lock().unwrap();
        hits.iter_mut().for_each(|h| h.score += usage.boost(&h.path));
    }
    let pins = app.pins.lock().unwrap();
    hits.iter_mut().for_each(|h| h.pinned = pins.has(&h.path));
    // pinned results first, each group by score
    hits.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.score.total_cmp(&a.score)));
    hits.truncate(20);
    hits.iter_mut().for_each(|h| h.hl = engine::highlight(&query, &h.name));
    hits
}

/// Seconds since 1970, for the lists of what you open.
fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// Pin a result so it is listed first, or unpin it.
fn pin(h: &Hit) {
    let mut pins = app().pins.lock().unwrap();
    if pins.has(&h.path) {
        pins.forget(&h.path);
    } else {
        pins.record(&h.path, &h.name, h.is_dir, h.app, now());
    }
}

/// Take an item out of the recent list (and out of the habit boost); no path clears them all.
fn forget(path: Option<&str>) {
    let mut usage = app().usage.lock().unwrap();
    match path {
        Some(p) => usage.forget(p),
        None => usage.clear(),
    }
}

/// (indexing in progress, item count, helper reachable)
fn status() -> (bool, usize, bool) {
    let app = app();
    let (busy, count) = (app.busy.load(Ordering::SeqCst) > 0, service::count(&app.network));
    match service::query("") {
        Some(r) => (busy || r.busy, count + r.count, true),
        None => (busy, count, false),
    }
}

/// folder=true reveals the item in Explorer, otherwise opens it with its default app.
fn open(h: &Hit, folder: bool) {
    let path = &h.path;
    app().usage.lock().unwrap().record(path, &h.name, h.is_dir, h.app, now());
    if folder {
        let _ = std::process::Command::new("explorer").arg(format!("/select,{path}")).spawn();
    } else {
        // ShellExecute, like double-clicking in Explorer: no console window flashing up (unlike `cmd /c start`).
        let (verb, file) = (engine::wide("open"), engine::wide(path));
        unsafe { windows_sys::Win32::UI::Shell::ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1 /* SW_SHOWNORMAL */) };
    }
}

/// The action menu: "copy" / "cut" (the file, as in Explorer), "path", "openas", "terminal", "properties".
fn act(action: &str, h: &Hit) {
    match action {
        "copy" | "cut" => { actions::copy_file(&h.path, action == "cut"); }
        "path" => { actions::copy_text(&h.path); }
        "terminal" => actions::terminal(&h.path, h.is_dir),
        v @ ("openas" | "properties") => actions::verb(v, &h.path),
        _ => {}
    }
}

/// Italian on an Italian Windows UI, else English. LISDIR_LANG=it|en overrides (handy for testing).
fn italian() -> bool {
    match (settings::LANGUAGE.load(Ordering::SeqCst), std::env::var("LISDIR_LANG").as_deref()) {
        (1, _) => false,
        (2, _) => true,
        (_, Ok(l)) => l == "it",
        _ => (unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() }) & 0x3ff == 0x10, // LANG_ITALIAN
    }
}

fn t(en: &'static str, it: &'static str) -> &'static str {
    if italian() { it } else { en }
}

/// Apply and save settings from the settings window. Errors are shown there, so they are plain sentences. The answer
/// says whether a restart is needed for the indexing choices (disks, folders, rules) to take effect.
fn save_settings(new: settings::Settings) -> Result<bool, String> {
    let new = new.checked()?;
    let app = app();
    let old = app.settings.read().unwrap().clone();
    if new.hotkey != old.hotkey {
        tray::set_hotkey(&new.hotkey).map_err(|e| format!("shortcut: {e}"))?;
    }
    new.save().map_err(|e| format!("cannot save: {e}"))?;
    new.apply_language();
    typesearch::ENABLED.store(new.type_search, Ordering::SeqCst);
    tray::set_visible(!new.hide_tray);
    peer::set_peers(new.peers.iter().filter_map(|l| peer::parse_peer(l)).collect()); // the list applies at once
    let restart = new.index_differs(&app.launched) || new.peers_differ(&app.launched);
    *app.settings.write().unwrap() = new;
    bar::refresh();
    Ok(restart)
}

fn autostart() -> bool {
    reg(&["query", RUN_KEY, "/v", "lisdir"])
}

/// Restart lisdir (and its helper) so the indexing choices apply. The new copy waits for this one to exit.
fn restart_app() {
    let app = app();
    let wipe = app.settings.read().unwrap().no_index != app.launched.no_index;
    service::stop_helper_and_wait();
    if wipe {
        engine::wipe_local_caches(); // what a removed or changed "do not index" rule had left out must be found again
    }
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg("--restart-after").arg(std::process::id().to_string()).spawn();
    }
    tray::quit();
}

/// `--restart-after <pid>`: wait (up to 10 s) for the copy that started us to exit, so its shortcut is free.
fn wait_for_exit(pid: u32) {
    use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::{OpenProcess, WaitForSingleObject}};
    unsafe {
        let h = OpenProcess(0x0010_0000 /* SYNCHRONIZE */, 0, pid);
        if !h.is_null() {
            WaitForSingleObject(h, 10_000);
            CloseHandle(h);
        }
    }
}

/// Open the list of extra network folders to index (UNC paths); changes apply on the next start.
fn edit_roots() {
    let (dir, f) = (engine::data_dir(), engine::data_dir().join("roots.txt"));
    let _ = std::fs::create_dir_all(&dir);
    if !f.exists() {
        let _ = std::fs::write(&f, t(
            "# One network folder per line, e.g. \\\\nas\\share. Mapped drives are found automatically.\r\n# Restart lisdir after editing.\r\n",
            "# Una cartella di rete per riga, per esempio \\\\nas\\condivisione. Le unità mappate vengono trovate da sole.\r\n# Riavvia lisdir dopo le modifiche.\r\n",
        ));
    }
    let _ = std::process::Command::new("notepad").arg(f).spawn();
}

fn reg(args: &[&str]) -> bool {
    std::process::Command::new("reg").args(args).creation_flags(engine::NO_WINDOW).status().is_ok_and(|s| s.success())
}

fn set_autostart(on: bool) {
    let exe = std::env::current_exe().map(|e| format!("\"{}\"", e.display())).unwrap_or_default();
    if on {
        reg(&["add", RUN_KEY, "/v", "lisdir", "/t", "REG_SZ", "/d", &exe, "/f"]);
    } else {
        reg(&["delete", RUN_KEY, "/v", "lisdir", "/f"]);
    }
}

/// Ask GitHub for a newer release and show the answer in the tray menu. Background checks stay quiet unless something is found.
fn check_update(manual: bool) {
    if manual {
        tray::set_update(t("Checking for updates…", "Controllo aggiornamenti…").into(), false);
    }
    match update::check() {
        Ok(Some(u)) => {
            tray::set_update(format!("{} {}…", t("Update to", "Aggiorna a"), u.version), true);
            *app().update.lock().unwrap() = Some(u);
        }
        Ok(None) if manual => tray::set_update(t("lisdir is up to date", "lisdir è aggiornato").into(), true),
        Err(_) if manual => tray::set_update(t("Update check failed – retry", "Controllo non riuscito – riprova").into(), true),
        _ => {}
    }
}

/// Download and start the installer (or swap in the new portable exe) for the update the user accepted, then quit
/// so the new version can take over.
fn install_update() {
    let Some(u) = app().update.lock().unwrap().take() else { return };
    tray::set_update(format!("{} {}…", t("Downloading", "Scarico"), u.version), false);
    match update::install(&u) {
        Ok(()) => tray::quit(),
        Err(_) => {
            tray::set_update(t("Update failed – retry", "Aggiornamento non riuscito – riprova").into(), true);
            *app().update.lock().unwrap() = Some(u);
        }
    }
}

/// The update item of the tray menu: install what was found, else look.
fn update_clicked() {
    if app().update.lock().unwrap().is_some() { install_update() } else { check_update(true) }
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--helper") => return service::run_helper(),
        Some("--install") => return service::install(),
        _ => {}
    }
    let args: Vec<String> = std::env::args().collect();
    if let Some(pid) = args.iter().position(|a| a == "--restart-after").and_then(|i| args.get(i + 1)).and_then(|p| p.parse().ok()) {
        wait_for_exit(pid);
    }
    update::cleanup();
    let loaded = settings::Settings::load();
    rules::set(&loaded.no_index, &loaded.exclude);
    typesearch::ENABLED.store(loaded.type_search, Ordering::SeqCst);
    let _ = APP.set(App { settings: RwLock::new(loaded.clone()), launched: loaded.clone(), ..Default::default() });
    let _ = unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED) }; // the folder chooser
    bar::create();
    tray::start(&loaded);
    start(app());
    if loaded.peers_on {
        peer::start(
            loaded.peers.iter().filter_map(|l| peer::parse_peer(l)).collect(),
            Arc::new(|q| shared_files(app(), q)),
            Arc::new(|name, code| {
                // a PC was just paired: keep it in the settings
                let mut s = app().settings.write().unwrap();
                s.add_peer(name, code);
                let _ = s.save();
            }),
        );
    }
    typesearch::start();
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_secs(20)); // let startup settle first
        if app().settings.read().unwrap().check_updates {
            check_update(false);
        }
        std::thread::sleep(std::time::Duration::from_secs(24 * 3600));
    });
    if std::env::args().any(|a| a == "--settings") {
        prefs::open(); // `lisdir.exe --settings`
    }
    unsafe {
        let mut m = std::mem::zeroed();
        while GetMessageW(&mut m, std::ptr::null_mut(), 0, 0) > 0 {
            if !bar::key(&m) && !prefs::key(&m) {
                TranslateMessage(&m);
                DispatchMessageW(&m);
            }
        }
    }
}
