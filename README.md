# lisdir

**English** · [Italiano](README.it.md)

A fast, open-source file search bar for Windows. Press **Alt+Space** (or just start typing in an
Explorer window or on the desktop) and find any file or folder on your machine as you type.

Written in Rust with a [Tauri](https://tauri.app) front end. MIT licensed.

## Features

- **Instant search over millions of files.** Local NTFS volumes are indexed by reading the MFT directly,
  not by walking directories, so a full index of a large disk takes seconds.
- **Always current.** The index follows the NTFS USN journal: files you create, rename, move or delete show up
  within about a second. The index is cached on disk, so later starts are near-instant.
- **Network folders.** Mapped drives and UNC paths (SMB shares) are indexed by a parallel directory walk and
  refreshed every 10 minutes.
- **Type to search.** With an Explorer folder or the desktop focused, typing letters opens the bar with those
  letters already entered (toggle it in the tray menu).
- **Fuzzy matching.** `lsrysvc` finds `Listary.Service.exe`; word starts and consecutive letters score higher.
- **App launcher.** Start menu and Store apps appear in the same results as files: type `calc`, press `Enter`.
- **File-type icons.** Results show the same icons as Explorer, so videos, photos, documents and archives stand out.
- **Keyboard driven.** `Enter` opens, `Ctrl+Enter` reveals in Explorer, `Esc` closes.
- **No admin UI.** The bar runs as a normal user; only a small background helper is elevated.
- **English and Italian.** The language follows your Windows display language (set `LISDIR_LANG=it` or `en` to
  override). The installer is translated too.

## How it works

```
 normal user                         elevated (scheduled task, runs at logon)
┌──────────────────────┐  named pipe ┌────────────────────────────┐
│ lisdir.exe           │◀───────────▶│ lisdir.exe --helper        │
│  Tauri window, tray, │  (ACL: you  │  MFT scan, USN journal,    │
│  hotkey, key hook,   │   + SYSTEM) │  on-disk index cache       │
│  SMB directory walk  │             └────────────────────────────┘
└──────────────────────┘
```

Reading a volume's MFT needs a raw volume handle, which needs administrator rights. Instead of running the
whole app elevated (which would make every file you open run elevated too), a windowless helper owns the NTFS
indexes and answers queries over a named pipe restricted to your user account. The UI also indexes network
folders itself, because only your user session has your credentials and drive mappings.

| File | Role |
|---|---|
| `src-tauri/src/engine.rs` | MFT enumeration, USN journal tailing, cache, directory walker, fuzzy matcher |
| `src-tauri/src/service.rs` | Elevated helper, pipe server/client, scheduled-task install |
| `src-tauri/src/typesearch.rs` | Low-level keyboard hook for system-wide "type to search" |
| `src-tauri/src/main.rs` | UI process: window, tray, hotkey, commands |
| `ui/index.html` | The search bar (plain HTML/JS, no build step) |

## Build

Requirements: Windows 10/11, the [Rust toolchain](https://rustup.rs) (MSVC), Visual Studio Build Tools
(C++ workload), and the WebView2 runtime (already present on Windows 11).

```
cd src-tauri
cargo test
cargo build --release
```

The binary is `src-tauri/target/release/lisdir.exe`. There is no front-end build step.

## Install and run

**Installer (recommended).** Download `lisdir-setup-<version>.exe` from the
[Releases](../../releases) page and run it. It asks for administrator rights once: to install into Program Files
and to register the scheduled task that starts the elevated helper at logon. Then press **Alt+Space**.
Uninstall from Windows Settings; this also removes the task. (The installer assumes you are the same account
that is signing in; installing with a different admin account's credentials is not supported yet.)

**Portable.** Run `lisdir-<version>-portable.exe`, then from the tray icon menu choose
**Install index service (admin)…** and accept the UAC prompt, once.

The executables are not code-signed yet, so SmartScreen may warn: *More info → Run anyway*.

Tray menu options: type-to-search on/off, start with Windows, edit network folders, install the service.
Extra network folders go in `%LOCALAPPDATA%\lisdir\roots.txt`, one path per line.

To build the installer yourself: install [Inno Setup 6](https://jrsoftware.org/isinfo.php), build the release
binary, then run `ISCC /DMyVersion=<version> installer\lisdir.iss` (output in `dist\`). Pushing a `v*` tag
does all of this in CI and publishes a GitHub release.

## Limitations and roadmap

- NTFS only for local disks (ReFS, exFAT and FAT volumes are not indexed).
- Network folders are re-walked rather than watched, so changes appear after the next refresh.
- The scheduled task points at the exe's current path; moving or rebuilding it while the helper runs
  requires stopping the helper first (the installer handles this on upgrade).
- Type-to-search does not check whether something is selected, and does not work over elevated windows.
- Not yet implemented: match highlighting, file icons, configurable hotkey, Open/Save dialog integration,
  content search.

## Contributing

Issues and pull requests are welcome. Run `cargo test` before sending a change, and note user-visible changes
under **Unreleased** in [CHANGELOG.md](CHANGELOG.md). Keep the engine free of UI
concerns: `engine.rs` should stay testable without a window or elevation.

## License

MIT, see [LICENSE](LICENSE). This is an independent project and is not affiliated with any other file
search tool.
