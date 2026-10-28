#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod apps;
mod engine;
mod icons;
mod service;
mod typesearch;

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
use tauri_plugin_global_shortcut::{Builder as Shortcuts, ShortcutState};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// This process runs unelevated and only indexes network folders (it has the user's credentials and
/// drive mappings). Local NTFS volumes are searched through the elevated helper, see `service`.
#[derive(Default)]
struct App {
    network: Volumes,
    /// Start menu and Store apps, loaded in the background at startup
    apps: Arc<RwLock<Vec<apps::App>>>,
    /// rendered icons by `icons::key`. ponytail: unbounded, but one entry per file type (plus one per program/shortcut shown).
    icons: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
    /// number of network walks in progress; non-zero shows "Indexing…"
    busy: Arc<AtomicUsize>,
    /// set once the window really got focus after being shown; blur before that is a foreground-lock glitch, not a click-away
    focused: AtomicBool,
}

/// Keep a network volume current by periodically re-walking it. It is searchable from the cache
/// (or empty) right away; a first walk of a big share can take minutes.
// ponytail: results of a walk appear only when it completes; stream into the index if first-run latency matters.
fn refresh(vol: Shared, mut stale: bool, busy: Arc<AtomicUsize>) {
    let root = vol.read().unwrap().root.clone();
    loop {
        if stale {
            busy.fetch_add(1, Ordering::SeqCst);
            let idx = engine::walk(&root);
            let mut v = vol.write().unwrap();
            v.idx = idx;
            v.dirty = true;
            let _ = engine::save(&mut v);
            drop(v);
            busy.fetch_sub(1, Ordering::SeqCst);
        }
        std::thread::sleep(engine::WALK_INTERVAL);
        stale = true;
    }
}

/// Start the network walks and make sure the elevated helper is running.
fn start(app: &App) {
    std::thread::spawn(|| {
        if service::query("").is_none() {
            service::start_helper(); // no-op (fails quietly) until the service has been installed
        }
    });
    let apps = app.apps.clone();
    std::thread::spawn(move || *apps.write().unwrap() = apps::load());
    for root in engine::network_roots() {
        let (v, fresh) = engine::open_walk(&root);
        let v: Shared = Arc::new(RwLock::new(v));
        app.network.write().unwrap().push(v.clone());
        let busy = app.busy.clone();
        std::thread::spawn(move || refresh(v, !fresh, busy));
    }
}

#[tauri::command]
async fn search(q: String, app: State<'_, App>) -> Result<Vec<Hit>, String> {
    let mut hits = service::search_volumes(&app.network, &q);
    if !q.trim().is_empty() {
        hits.extend(service::query(&q).map(|r| r.hits).unwrap_or_default());
        // apps are ranked together with files; their Start menu shortcuts would only duplicate them
        hits.retain(|h| !(h.path.contains(r"\Start Menu\Programs\") && h.path.to_lowercase().ends_with(".lnk")));
        hits.extend(apps::search(&app.apps.read().unwrap(), &q, 20).into_iter().map(|(score, a)| Hit::app(&a.name, &a.id, score)));
    }
    hits.sort_by(|a, b| b.score.total_cmp(&a.score));
    hits.truncate(20);
    Ok(hits)
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
fn open(path: String, folder: bool) {
    if folder {
        let _ = std::process::Command::new("explorer").arg(format!("/select,{path}")).spawn();
    } else {
        // ShellExecute, like double-clicking in Explorer: no console window flashing up (unlike `cmd /c start`).
        let (verb, file): (Vec<u16>, Vec<u16>) = ("open\0".encode_utf16().collect(), path.encode_utf16().chain([0]).collect());
        unsafe { windows_sys::Win32::UI::Shell::ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1 /* SW_SHOWNORMAL */) };
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
    match std::env::var("LISDIR_LANG").as_deref() {
        Ok(l) => l == "it",
        Err(_) => (unsafe { windows_sys::Win32::Globalization::GetUserDefaultUILanguage() }) & 0x3ff == 0x10, // LANG_ITALIAN
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
            let _ = w.show();
            let _ = w.set_focus();
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

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--helper") => return service::run_helper(),
        Some("--install") => return service::install(),
        _ => {}
    }
    tauri::Builder::default()
        .plugin(
            Shortcuts::new()
                .with_shortcuts(["alt+space"])
                .unwrap()
                .with_handler(|app, _, e| {
                    if e.state() == ShortcutState::Pressed {
                        toggle(app)
                    }
                })
                .build(),
        )
        .manage(App::default())
        .setup(|app| {
            start(&app.state::<App>());
            typesearch::start(app.handle().clone());
            let type_search = CheckMenuItem::with_id(app, "typesearch", t("Type to search in Explorer/Desktop", "Digita per cercare in Esplora risorse/Desktop"), true, true, None::<&str>)?;
            let autostart = CheckMenuItem::with_id(app, "autostart", t("Start with Windows", "Avvia con Windows"), true, reg(&["query", RUN_KEY, "/v", "lisdir"]), None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &MenuItem::with_id(app, "version", format!("lisdir {VERSION}"), false, None::<&str>)?,
                    &MenuItem::with_id(app, "open", t("Open (Alt+Space)", "Apri (Alt+Spazio)"), true, None::<&str>)?,
                    &type_search,
                    &autostart,
                    &MenuItem::with_id(app, "roots", t("Network folders…", "Cartelle di rete…"), true, None::<&str>)?,
                    &MenuItem::with_id(app, "install", t("Install index service (admin)…", "Installa servizio di indicizzazione (admin)…"), true, None::<&str>)?,
                    &MenuItem::with_id(app, "quit", t("Quit", "Esci"), true, None::<&str>)?,
                ],
            )?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip(format!("lisdir {VERSION}"))
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(move |app, e| match e.id.as_ref() {
                    "open" => toggle(app),
                    "typesearch" => typesearch::ENABLED.store(type_search.is_checked().unwrap_or(true), Ordering::SeqCst),
                    "autostart" => set_autostart(autostart.is_checked().unwrap_or(false)),
                    "roots" => edit_roots(),
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
            Ok(())
        })
        .on_window_event(|w, e| {
            let app = w.state::<App>();
            match e {
                WindowEvent::Focused(true) => app.focused.store(true, Ordering::SeqCst),
                WindowEvent::Focused(false) if app.focused.swap(false, Ordering::SeqCst) => {
                    let _ = w.hide();
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![search, status, open, hide, take_pending, version, lang, icon])
        .run(tauri::generate_context!())
        .expect("tauri");
}
