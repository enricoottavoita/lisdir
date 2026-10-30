# lisdir

**English** · [Italiano](README.it.md)

A fast, open-source search bar and app launcher for Windows. Press **Alt+Space** (or just start typing in an
Explorer window or on the desktop) and find files, folders and apps on your machine as you type.

Written in Rust with a [Tauri](https://tauri.app) front end. MIT licensed.

## Features

- **Instant search over millions of files.** Local NTFS volumes are indexed by reading the MFT directly,
  not by walking directories, which is far faster than scanning folders. Measured on 1.6 million entries
  (two disks): a text search takes about 10–25 ms (30–40 ms for a single letter); filters such as `date:` and `size:` about 8–10 ms.
- **Always current.** The index follows the NTFS USN journal (checked every second): files you create, rename,
  move or delete show up within a couple of seconds, and so do edits (the modified time and size are refreshed when
  a file is closed). The index is cached on disk, so a restart only has to catch up on what changed; between
  restarts only the changes are appended to a small log, not the whole cache rewritten.
- **Network folders and other disks.** Mapped drives, UNC paths (SMB shares) and local disks that are not NTFS (USB
  sticks, exFAT, FAT) are indexed by a parallel directory walk and refreshed every 10 minutes. A stick keeps its
  results while unplugged. On the very first walk, results appear while it is still running.
- **Type to search.** With an Explorer folder or the desktop focused, typing letters opens the bar with those
  letters already entered (switch it off in the tray menu or the settings).
- **Fuzzy matching.** `rdme` finds `README.md`; word starts and consecutive letters score higher. Several words
  match in any order, and the matched letters are highlighted.
- **Search syntax.** Narrow with `ext:pdf`, `date:week`, `size:>1gb`, `-draft`, `"exact phrase"`, `docs\ report`; see below.
- **Recent items.** Open the bar and type nothing to see your pinned items, then what you opened lately, under a
  grey "Recently opened" heading so it reads as history (`Del`
  removes one, `Shift+Del` clears the list). Things you open often rank a little higher, but how well the name
  matches always counts most.
- **App launcher.** Start menu and Store apps appear in the same results as files: type `calc`, press `Enter`. The
  list is refreshed when the bar opens, so new installs show up without a restart.
- **File-type icons and sizes.** Results show the same icons as Explorer, and the size at the right: for a folder, the
  total of the files inside it, kept current as they change.
- **Keyboard driven.** `Enter` opens, `Ctrl+Enter` reveals in Explorer, `Esc` closes, `Ctrl+,` opens the settings.
- **Actions on a result.** `→`, `Ctrl+O` or a right click opens a menu: copy or cut the file (`Ctrl+C` / `Ctrl+X`,
  then paste it in Explorer), copy its path (`Ctrl+Shift+C`), open with, open a terminal there (`Ctrl+T`),
  properties (`Alt+Enter`). The shortcuts also work with the menu closed.
- **Preview and pins.** `Alt+P` shows a preview pane: the start of a text file, or a thumbnail. `Ctrl+P` pins a
  result so it is always listed first.
- **Other PCs.** Optional and off by default: pair two PCs that run lisdir (home and office, say) and each one also
  lists files found on the other, marked with its name. They cannot be opened from afar, only their path copied.
  Encrypted, direct when the networks allow it; no account. See below.
- **Settings.** The shortcut that opens the bar, language, colour theme (Windows, Dark, Light, Midnight, Paper),
  start with Windows, and what is indexed: which disks, extra folders (with a folder chooser), and rules for what to
  leave out of the index or hide from the results.
- **Updates.** The tray menu offers a new version when one is published and installs it after you click, checking
  its SHA-256 first (a portable copy updates itself in place). This check is the only internet request lisdir
  makes, and it can be turned off in the settings.
- **No admin UI.** The bar runs as a normal user; only a small background helper is elevated.
- **English and Italian.** The language follows your Windows display language, or choose it in the settings (for
  tests, `LISDIR_LANG=it` or `en` also works). The installer is translated too.

## Search syntax

Plain words match file names; everything else narrows the results. All of it can be combined.

| You type | It means |
|---|---|
| `report 2024` | names containing both words, in any order (fuzzy) |
| `ext:pdf;docx` or `*.pdf` | only these extensions |
| `-draft` / `-ext:tmp` | name must not contain `draft` / not this extension |
| `"annual report"` | the exact phrase in the name |
| `docs\ report` | "report" inside a folder whose name contains "docs" |
| `folder:` / `file:` | only folders / only files |
| `doc:` `pic:` `video:` `music:` `zip:` | a whole category of file types |
| `date:today`, `yesterday`, `week`, `month`, `year` | modified today / yesterday / in the last 7, 30, 365 days |
| `date:2026`, `date:2026-07`, `date:2026-07-31` | modified in that year / month / day |
| `date:3d`, `2w`, `6m`, `1y` | modified in the last 3 days / 2 weeks / 6 months / 1 year |
| `date:monday` (or `mon`) | modified on the most recent Monday (today, if today is Monday) |
| `date:lastweek`, `lastmonth`, `lastyear` | modified in the previous calendar week (Mon–Sun) / month / year |
| `size:>1gb`, `size:<10mb`, `size:10mb..1gb`, `size:empty` | file size (`>` `>=` `<` `<=`, a range, or empty); units kb mb gb tb |
| `app:` | apps only |

`date:` and `size:` use the file's modified time and size, read from the NTFS file record (for network folders, from
the directory listing). A size filter matches files only. Words and filters together work too:
`report ext:pdf date:month -draft`. With only filters and no words, the newest files come first; with `size:`, the
biggest.

## How it works

```
 normal user                         elevated (scheduled task, runs at logon)
┌──────────────────────┐  named pipe ┌────────────────────────────┐
│ lisdir.exe           │◀───────────▶│ lisdir.exe --helper        │
│  Tauri window, tray, │  (ACL: you  │  MFT scan, USN journal,    │
│  hotkey, key hook,   │   + SYSTEM) │  on-disk index cache       │
│  SMB walk, app list  │             └────────────────────────────┘
└──────────────────────┘
```

Reading a volume's MFT needs a raw volume handle, which needs administrator rights. Instead of running the
whole app elevated (which would make every file you open run elevated too), a windowless helper owns the NTFS
indexes and answers queries over a named pipe restricted to your user account. The UI also indexes network
folders itself, because only your user session has your credentials and drive mappings.

| File | Role |
|---|---|
| `src-tauri/src/engine.rs` | The index (flat list of entries), MFT enumeration, USN journal tailing, cache and change log, directory walker, fuzzy matcher, parallel search |
| `src-tauri/src/mft.rs` | Reads modified time and size out of a raw NTFS file record |
| `src-tauri/src/service.rs` | Elevated helper, pipe server/client, scheduled-task install |
| `src-tauri/src/query.rs` | The search syntax: parsing `ext:`, `date:`, quotes, `docs\`, ... |
| `src-tauri/src/peer.rs` | Other PCs: the pairing list, the encrypted link (iroh/QUIC), what this PC answers and the questions it asks |
| `src-tauri/src/usage.rs` | What you open: recent items and the small ranking boost; the same list format holds the pins |
| `src-tauri/src/apps.rs` | Start menu and Store app list for the launcher |
| `src-tauri/src/icons.rs` | Shell file-type icons and preview thumbnails rendered to BMP |
| `src-tauri/src/actions.rs` | The action menu: files on the clipboard as Explorer puts them, open with, terminal, properties; text for the preview |
| `src-tauri/src/settings.rs` | The settings file: shortcut, language, theme, tray icon, update check, indexed disks and folders, rules |
| `src-tauri/src/rules.rs` | The do-not-index and hide-from-results rules |
| `src-tauri/src/update.rs` | Update check against the GitHub releases; downloads and verifies the installer, or swaps in the portable exe |
| `src-tauri/src/typesearch.rs` | Low-level keyboard hook for system-wide "type to search" |
| `src-tauri/src/main.rs` | UI process: window, tray, hotkey, commands |
| `ui/index.html` | The search bar (plain HTML/JS, no build step) |
| `ui/settings.html` | The settings window |
| `ui/theme.css` | The colour themes, shared by the bar and the settings window |

## Build

Requirements: Windows 10/11 (64-bit), the [Rust toolchain](https://rustup.rs) (MSVC), Visual Studio Build Tools
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

Tray menu options: settings, type-to-search on/off, start with Windows, edit network folders, check for updates, install the service.

**Settings** (tray menu → Settings…, `Ctrl+,` in the bar, or `lisdir.exe --settings`): the shortcut that opens the
bar (it needs Ctrl, Alt or Win, or is a function key; one another program already uses is refused), language, colour
theme, hiding the tray icon, the daily update check, start with Windows, type to search, and what is indexed
(below). They are saved in `%LOCALAPPDATA%\lisdir\settings.txt`, plain `key=value` lines.

**Other PCs.** In the settings window, section *Other PCs*: switch it on, save and restart lisdir (on both PCs). Then
pair them: on one PC press **Allow a new PC** (open for two minutes), on the other paste the first PC's code and press
**Pair**. Both PCs show a 4-character verification code; the first one also shows who is asking, and you confirm there
only if the code is the same on both screens. Both PCs then list each other, with no restart. (The list can also be edited by hand as `Name|code` lines.) From then on a search also asks your
other PCs, and what they find appears after your own results with the PC's name in front. What travels: the search
text one way, up to 20 names, paths and sizes the other way, never the files. Only PCs on your list are answered; the
link is encrypted and tied to each PC's key (`peer.key`, stored sealed with Windows data protection; never share it). It connects directly when both networks
allow it and otherwise through a public relay that only carries encrypted bytes. A PC that is off is skipped, so
typing never waits for it. Nothing runs, and nothing connects, while the option is off. A badge next to the section
title says whether the option is active and how many PCs are connected, and the list below it shows each paired PC
as connected or not, with when it was last seen.

**What is indexed.** In the settings window you can untick a disk, add more folders to walk (the **Add folder…**
button opens the Windows folder chooser; or type one path per line, for example `\\nas\share`; the older
`%LOCALAPPDATA%\lisdir\roots.txt` still works), and write rules:

| Rule | Effect |
|---|---|
| **Do not index** | matching names and folders are left out of the index, with everything inside (saves memory) |
| **Hide from results** | matching names and folders stay indexed but are never shown |

A rule is a name or pattern (`node_modules`, `*.tmp`, `~$*`; `*` is any text, `?` one character) or the path of a
folder (`C:\Users\me\Cache`); case does not matter. Changes to disks, folders and rules apply after a restart; the
window has a **Restart now** button. Removing or changing a do-not-index rule rebuilds the local disks' index once.

**Updates.** lisdir checks the GitHub releases a few seconds after it starts and then daily; when a newer one
exists the tray menu offers "Update to X.Y.Z…" (or choose "Check for updates…" at any time). It downloads the
installer, verifies its SHA-256 against the published `SHA256SUMS.txt`, and starts it; Windows asks for
permission, and lisdir reopens afterwards. Nothing is installed without your click, and you can turn the daily
check off in the settings. A portable copy stays
portable: it downloads the new portable exe instead, verifies it the same way, puts it in place of the old file and
restarts (no permission prompt; the folder must be writable).

To build the installer yourself: install [Inno Setup 6](https://jrsoftware.org/isinfo.php), build the release
binary, then run `ISCC /DMyVersion=<version> installer\lisdir.iss` (output in `dist\`). Pushing a `v*` tag
does all of this in CI and publishes a GitHub release.

## Limitations and roadmap

- Local disks are read from the MFT only when they are NTFS; other local file systems (exFAT, FAT) are walked, which
  is slower to build and does not see changes until the next refresh. ReFS volumes and optical drives are not indexed.
- Network folders are re-walked rather than watched, so changes appear after the next refresh.
- Sizes and times on local disks come from the NTFS file records. A file whose data attribute sits in an extension
  record (a heavily fragmented or very attribute-rich file) reports size 0, and the size is not shown for it.
- The scheduled task points at the exe's current path; moving or rebuilding it while the helper runs
  requires stopping the helper first (the installer handles this on upgrade).
- A network folder on its very first walk shows partial results until the walk finishes (minutes for a very large
  share); later starts use the cache.
- Type-to-search does not check whether something is selected, and does not work over elevated windows.
- Setting up the elevated helper needs administrator rights once; after that the bar needs none.
- The first scan of a local disk reads one file record per file to collect sizes and times; that takes about 11
  seconds more per million files, once per disk (and again whenever the cache format changes).
- There is no way yet to sort results by size or date, and `size:` does not match folders.
- The text preview reads files as UTF-8: accented letters in older ANSI files show as `�`, and a UTF-16 file shows its icon.
- The action menu works on one result at a time (several at once comes with the search window).
- If a folder is renamed to a name a hide rule matches, the files already inside it are hidden only after the next
  restart. Path rules name folders, not single files.
- Not yet implemented: Open/Save dialog integration, content search.
  The plan is in [TODO.md](TODO.md).

## Contributing

Issues and pull requests are welcome. Run `cargo test` before sending a change, and note user-visible changes
under **Unreleased** in [CHANGELOG.md](CHANGELOG.md). Keep the engine free of UI
concerns: `engine.rs` should stay testable without a window or elevation.

## License

MIT, see [LICENSE](LICENSE). This is an independent project and is not affiliated with any other file
search tool.
