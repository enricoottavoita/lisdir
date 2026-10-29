#![cfg_attr(not(any(debug_assertions, test)), windows_subsystem = "windows")]
mod actions;
mod apps;
mod engine;
mod icons;
mod mft;
mod query;
mod rules;
mod service;
mod settings;
mod typesearch;
mod update;
mod usage;

use query::{Clock, Query};
use service::{Hit, Shared, Volumes};
use std::{
    os::windows::process::CommandExt,
    sync::{atomic::{AtomicBool, AtomicUsize, Ordering}, Arc, RwLock},
};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, State, WebviewWindow, WindowEvent,
};
use tauri_plugin_global_shortcut::{Builder as Shortcuts, GlobalShortcutExt, ShortcutState};

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
    /// rendered icons by `icons::key`. ponytail: unbounded, but one entry per file type (plus one per program/shortcut shown).
    icons: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
    /// number of network walks in progress; non-zero shows "Indexing…"
    busy: Arc<AtomicUsize>,
    /// set once the window really got focus after being shown; blur before that is a foreground-lock glitch, not a click-away
    focused: AtomicBool,
    /// a newer release found by the update check, waiting for the user to accept it
    update: std::sync::Mutex<Option<update::Update>>,
    settings: RwLock<settings::Settings>,
    /// the settings the index was built with; a difference in disks, folders or rules needs a restart to apply
    launched: settings::Settings,
}

/// The two tray check items, so that the settings window can keep them in step.
static CHECKS: std::sync::OnceLock<(tauri::menu::CheckMenuItem<tauri::Wry>, tauri::menu::CheckMenuItem<tauri::Wry>)> = std::sync::OnceLock::new();

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

#[tauri::command]
async fn search(q: String, app: State<'_, App>) -> Result<Vec<Hit>, String> {
    let query = Query::parse(&q, &Clock::system());
    if query.is_empty() {
        return Ok(recents(&app));
    }
    let mut hits = vec![];
    if query.searches_files() {
        hits = service::search_volumes(&app.network, &query);
        hits.extend(service::query(&q).map(|r| r.hits).unwrap_or_default());
        // apps are ranked together with files; their Start menu shortcuts would only duplicate them
        hits.retain(|h| !(h.path.contains(r"\Start Menu\Programs\") && h.path.to_lowercase().ends_with(".lnk")));
    }
    if query.wants_apps() {
        hits.extend(apps::search(&app.apps.read().unwrap(), &query, 20).into_iter().map(|(score, a)| Hit::app(&a.name, &a.id, score)));
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
    Ok(hits)
}

/// Pin a result so it is listed first, or unpin it.
#[tauri::command]
fn pin(path: String, name: String, is_dir: bool, app: bool, state: State<App>) {
    let mut pins = state.pins.lock().unwrap();
    if pins.has(&path) {
        pins.forget(&path);
    } else {
        pins.record(&path, &name, is_dir, app, std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64));
    }
}

/// Preview of a result: b't' + the start of a text file, or b'i' + a PNG (the shell's thumbnail or big icon).
#[tauri::command]
async fn preview(path: String, is_dir: bool) -> Result<tauri::ipc::Response, String> {
    Ok(tauri::ipc::Response::new(match (!is_dir).then(|| actions::text_start(&path)).flatten() {
        Some(text) => [b"t".as_slice(), text.as_bytes()].concat(),
        None => [b"i".as_slice(), &icons::thumbnail_png(&path, 512).ok_or("no preview")?].concat(),
    }))
}

/// Show or hide the preview pane: the bar gets wider so the list keeps its width.
#[tauri::command]
fn pane(on: bool, w: WebviewWindow) {
    let _ = w.set_size(tauri::LogicalSize::new(if on { 1100.0 } else { 700.0 }, 440.0));
    let _ = w.center();
}

/// Take an item out of the recent list (and out of the habit boost); no path clears them all.
#[tauri::command]
fn forget(path: Option<String>, app: State<App>) {
    let mut usage = app.usage.lock().unwrap();
    match path {
        Some(p) => usage.forget(&p),
        None => usage.clear(),
    }
}

/// (indexing in progress, item count, helper reachable)
#[tauri::command]
async fn status(app: State<'_, App>) -> Result<(bool, usize, bool), String> {
    let (busy, count) = (app.busy.load(Ordering::SeqCst) > 0, service::count(&app.network));
    Ok(match service::query("") {
        Some(r) => (busy || r.busy, count + r.count, true),
        None => (busy, count, false),
    })
}

/// folder=true reveals the item in Explorer, otherwise opens it with its default app.
#[tauri::command]
fn open(path: String, folder: bool, name: String, is_dir: bool, app: bool, state: State<App>) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    state.usage.lock().unwrap().record(&path, &name, is_dir, app, now);
    if folder {
        let _ = std::process::Command::new("explorer").arg(format!("/select,{path}")).spawn();
    } else {
        // ShellExecute, like double-clicking in Explorer: no console window flashing up (unlike `cmd /c start`).
        let (verb, file): (Vec<u16>, Vec<u16>) = ("open\0".encode_utf16().collect(), path.encode_utf16().chain([0]).collect());
        unsafe { windows_sys::Win32::UI::Shell::ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1 /* SW_SHOWNORMAL */) };
    }
}

/// The action menu: "copy" / "cut" (the file, as in Explorer), "path", "openas", "terminal", "properties".
#[tauri::command]
fn act(action: String, path: String, is_dir: bool, w: WebviewWindow) {
    let _ = w.hide(); // first, so the dialog or terminal comes to the front
    match action.as_str() {
        "copy" | "cut" => { actions::copy_file(&path, action == "cut"); }
        "path" => { actions::copy_text(&path); }
        "terminal" => actions::terminal(&path, is_dir),
        v @ ("openas" | "properties") => actions::verb(v, &path),
        _ => {}
    }
}

/// PNG of the shell icon for a result (`key` is `Hit::icon`). Raw bytes, no base64.
#[tauri::command]
async fn icon(key: String, path: String, app: State<'_, App>) -> Result<tauri::ipc::Response, String> {
    if let Some(png) = app.icons.lock().unwrap().get(&key) {
        return Ok(tauri::ipc::Response::new(png.clone()));
    }
    let png = icons::icon_png(&key, &path).ok_or("no icon")?;
    app.icons.lock().unwrap().insert(key, png.clone());
    Ok(tauri::ipc::Response::new(png))
}

#[tauri::command]
fn version() -> &'static str {
    VERSION
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

#[tauri::command]
fn lang() -> &'static str {
    t("en", "it")
}

#[tauri::command]
fn get_settings(app: State<App>) -> settings::Settings {
    app.settings.read().unwrap().clone()
}

/// Apply and save settings from the settings window. Errors are shown there, so they are plain sentences. The answer
/// says whether a restart is needed for the indexing choices (disks, folders, rules) to take effect.
#[tauri::command]
fn save_settings(new: settings::Settings, app: AppHandle) -> Result<bool, String> {
    let new = new.checked()?;
    let state = app.state::<App>();
    let old = state.settings.read().unwrap().clone();
    if new.hotkey != old.hotkey {
        // register the new one first, so a shortcut that is taken by another program leaves the old one working
        app.global_shortcut().register(new.hotkey.as_str()).map_err(|e| format!("shortcut: {e}"))?;
        let _ = app.global_shortcut().unregister(old.hotkey.as_str());
    }
    new.save().map_err(|e| format!("cannot save: {e}"))?;
    new.apply_language();
    typesearch::ENABLED.store(new.type_search, Ordering::SeqCst);
    if let Some((type_search, _)) = CHECKS.get() {
        let _ = type_search.set_checked(new.type_search);
    }
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_visible(!new.hide_tray);
    }
    let restart = new.index_differs(&state.launched);
    *state.settings.write().unwrap() = new;
    // the bar re-reads language and theme
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.eval("location.reload()");
    }
    Ok(restart)
}

#[tauri::command]
fn get_autostart() -> bool {
    reg(&["query", RUN_KEY, "/v", "lisdir"])
}

#[tauri::command]
fn set_autostart_cmd(on: bool) {
    set_autostart(on);
    if let Some((_, autostart)) = CHECKS.get() {
        let _ = autostart.set_checked(on);
    }
}

#[tauri::command]
fn list_drives(app: State<App>) -> Vec<engine::Drive> {
    engine::drives(&app.settings.read().unwrap().disks_off)
}

/// Restart lisdir (and its helper) so the indexing choices apply. The new copy waits for this one to exit.
#[tauri::command]
fn restart_app(app: AppHandle) {
    let state = app.state::<App>();
    let wipe = state.settings.read().unwrap().no_index != state.launched.no_index;
    service::stop_helper_and_wait();
    if wipe {
        engine::wipe_local_caches(); // what a removed or changed "do not index" rule had left out must be found again
    }
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::process::Command::new(exe).arg("--restart-after").arg(std::process::id().to_string()).spawn();
    }
    app.exit(0);
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

/// Open the settings window (or bring it forward).
fn open_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let title = t("lisdir settings", "Impostazioni di lisdir");
    let built = tauri::WebviewWindowBuilder::new(app, "settings", tauri::WebviewUrl::App("settings.html".into()))
        .title(title)
        .inner_size(560.0, 780.0)
        .resizable(false)
        .center()
        .build();
    if let Ok(w) = built {
        let _ = w.set_focus();
    }
}

/// The folder chooser of Windows, for the lists in the settings window. None if the user cancels.
#[tauri::command]
async fn pick_folder(w: WebviewWindow) -> Option<String> {
    rfd::FileDialog::new()
        .set_title(t("Choose a folder", "Scegli una cartella"))
        .set_parent(&w)
        .pick_folder()
        .map(|p| p.to_string_lossy().trim_end_matches('\\').to_string())
}

#[tauri::command]
fn close_window(w: WebviewWindow) {
    let _ = w.close();
}

#[tauri::command]
fn settings_window(app: AppHandle) {
    open_settings(&app);
}

#[tauri::command]
fn hide(w: WebviewWindow) {
    let _ = w.hide();
}

#[tauri::command]
fn take_pending() -> String {
    typesearch::take()
}

/// Show and focus the bar if it is hidden.
fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        if !w.is_visible().unwrap_or(false) {
            let _ = w.center();
            app.state::<App>().focused.store(false, Ordering::SeqCst);
            reload_apps(&app.state::<App>());
            let _ = w.show();
            let _ = w.set_focus();
            // Tell the page it was just opened (the DOM focus event is not reliable on the first show).
            let _ = w.eval("window.opened && window.opened()");
        }
    }
}

fn toggle(app: &AppHandle) {
    match app.get_webview_window("main") {
        Some(w) if w.is_visible().unwrap_or(false) => { let _ = w.hide(); }
        _ => show(app),
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
    std::process::Command::new("reg").args(args).creation_flags(0x0800_0000).status().is_ok_and(|s| s.success())
}

fn set_autostart(on: bool) {
    let exe = std::env::current_exe().map(|e| format!("\"{}\"", e.display())).unwrap_or_default();
    if on {
        reg(&["add", RUN_KEY, "/v", "lisdir", "/t", "REG_SZ", "/d", &exe, "/f"]);
    } else {
        reg(&["delete", RUN_KEY, "/v", "lisdir", "/f"]);
    }
}

type Item = MenuItem<tauri::Wry>;

fn set_item(item: &Item, text: String, enabled: bool) {
    let _ = item.set_text(text);
    let _ = item.set_enabled(enabled);
}

/// Ask GitHub for a newer release and show the answer in the tray menu. Background checks stay quiet unless something is found.
fn check_update(app: &AppHandle, item: &Item, manual: bool) {
    if manual {
        set_item(item, t("Checking for updates…", "Controllo aggiornamenti…").into(), false);
    }
    match update::check() {
        Ok(Some(u)) => {
            set_item(item, format!("{} {}…", t("Update to", "Aggiorna a"), u.version), true);
            *app.state::<App>().update.lock().unwrap() = Some(u);
        }
        Ok(None) if manual => set_item(item, t("lisdir is up to date", "lisdir è aggiornato").into(), true),
        Err(_) if manual => set_item(item, t("Update check failed – retry", "Controllo non riuscito – riprova").into(), true),
        _ => {}
    }
}

/// Download and start the installer (or swap in the new portable exe) for the update the user accepted, then quit
/// so the new version can take over.
fn install_update(app: &AppHandle, item: &Item) {
    let Some(u) = app.state::<App>().update.lock().unwrap().take() else { return };
    set_item(item, format!("{} {}…", t("Downloading", "Scarico"), u.version), false);
    match update::install(&u) {
        Ok(()) => app.exit(0),
        Err(_) => {
            set_item(item, t("Update failed – retry", "Aggiornamento non riuscito – riprova").into(), true);
            *app.state::<App>().update.lock().unwrap() = Some(u);
        }
    }
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
    loaded.apply_rules();
    typesearch::ENABLED.store(loaded.type_search, Ordering::SeqCst);
    tauri::Builder::default()
        .plugin(
            Shortcuts::new()
                .with_handler(|app, _, e| {
                    if e.state() == ShortcutState::Pressed {
                        toggle(app)
                    }
                })
                .build(),
        )
        .manage(App { settings: RwLock::new(loaded.clone()), launched: loaded.clone(), ..Default::default() })
        .setup(move |app| {
            // the saved shortcut, or Alt+Space when it is taken by another program
            if app.global_shortcut().register(loaded.hotkey.as_str()).is_err() {
                let _ = app.global_shortcut().register("alt+space");
            }
            start(&app.state::<App>());
            typesearch::start(app.handle().clone());
            let type_search = CheckMenuItem::with_id(app, "typesearch", t("Type to search in Explorer/Desktop", "Digita per cercare in Esplora risorse/Desktop"), true, loaded.type_search, None::<&str>)?;
            let autostart = CheckMenuItem::with_id(app, "autostart", t("Start with Windows", "Avvia con Windows"), true, reg(&["query", RUN_KEY, "/v", "lisdir"]), None::<&str>)?;
            let _ = CHECKS.set((type_search.clone(), autostart.clone()));
            let update_item = MenuItem::with_id(app, "update", t("Check for updates…", "Controlla aggiornamenti…"), true, None::<&str>)?;
            let (handle, item) = (app.handle().clone(), update_item.clone());
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(20)); // let startup settle first
                if handle.state::<App>().settings.read().unwrap().check_updates {
                    check_update(&handle, &item, false);
                }
                std::thread::sleep(std::time::Duration::from_secs(24 * 3600));
            });
            let menu = Menu::with_items(
                app,
                &[
                    &MenuItem::with_id(app, "version", format!("lisdir {VERSION}"), false, None::<&str>)?,
                    &MenuItem::with_id(app, "open", t("Open (Alt+Space)", "Apri (Alt+Spazio)"), true, None::<&str>)?,
                    &type_search,
                    &autostart,
                    &MenuItem::with_id(app, "settings", t("Settings…", "Impostazioni…"), true, None::<&str>)?,
                    &MenuItem::with_id(app, "roots", t("Network folders…", "Cartelle di rete…"), true, None::<&str>)?,
                    &update_item,
                    &MenuItem::with_id(app, "install", t("Install index service (admin)…", "Installa servizio di indicizzazione (admin)…"), true, None::<&str>)?,
                    &MenuItem::with_id(app, "quit", t("Quit", "Esci"), true, None::<&str>)?,
                ],
            )?;
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip(format!("lisdir {VERSION}"))
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, e| match e.id.as_ref() {
                    "open" => toggle(app),
                    "typesearch" => {
                        let on = type_search.is_checked().unwrap_or(true);
                        typesearch::ENABLED.store(on, Ordering::SeqCst);
                        let state = app.state::<App>();
                        let mut s = state.settings.write().unwrap();
                        s.type_search = on;
                        let _ = s.save();
                    }
                    "autostart" => set_autostart(autostart.is_checked().unwrap_or(false)),
                    "settings" => open_settings(app),
                    "roots" => edit_roots(),
                    "update" => {
                        let (app, item) = (app.clone(), update_item.clone());
                        std::thread::spawn(move || {
                            if app.state::<App>().update.lock().unwrap().is_some() { install_update(&app, &item) } else { check_update(&app, &item, true) }
                        });
                    }
                    "install" => service::request_install(),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, e| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                        toggle(tray.app_handle())
                    }
                })
                .build(app)?;
            if std::env::args().any(|a| a == "--settings") {
                open_settings(app.handle()); // `lisdir.exe --settings`
            }
            if loaded.hide_tray {
                if let Some(tray) = app.tray_by_id("main") {
                    let _ = tray.set_visible(false);
                }
            }
            Ok(())
        })
        .on_window_event(|w, e| {
            if w.label() != "main" {
                return; // only the search bar hides itself when it loses focus
            }
            let app = w.state::<App>();
            match e {
                WindowEvent::Focused(true) => app.focused.store(true, Ordering::SeqCst),
                WindowEvent::Focused(false) if app.focused.swap(false, Ordering::SeqCst) => {
                    let _ = w.hide();
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![search, status, open, act, pin, preview, pane, hide, take_pending, version, lang, icon, forget, get_settings, save_settings, settings_window, close_window, pick_folder, get_autostart, set_autostart_cmd, list_drives, restart_app])
        .run(tauri::generate_context!())
        .expect("tauri");
}
