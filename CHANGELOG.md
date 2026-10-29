# Changelog

All notable changes to lisdir are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

When you tag a release `vX.Y.Z`, the release workflow publishes the matching `## [X.Y.Z]` section below as the
release notes, and fails if that section is missing. Add changes under **Unreleased** as you go, then rename it
when releasing.

## [Unreleased]

## [0.16.0] - 2026-10-29

### Added
- Settings window (tray menu → **Settings…**, `Ctrl+,` in the bar, or `lisdir.exe --settings`): the shortcut that
  opens the bar (any combination with Ctrl, Alt or Win, or a function key; a plain letter or `Space` is refused, and
  so is a shortcut another program already uses), language (Windows, English, Italiano), colour theme (Windows,
  Dark, Light, Midnight, Paper), hiding the tray icon, and turning the daily update check off. They are kept in
  `%LOCALAPPDATA%\lisdir\settings.txt` (plain `key=value` lines). Start with Windows and Type to search are in the
  window too, and stay in step with the tray menu (Type to search is now remembered between runs).
- Choose what is indexed, in the settings window: untick a disk to leave it out; list more folders to index (a
  network share or any folder, walked every 10 minutes); **Do not index** rules (left out of the index, with
  everything inside); **Hide from results** rules (still indexed, never shown). A rule is a name or pattern
  (`node_modules`, `*.tmp`, `~$*`) or a folder path (`C:\Users\me\Cache`); case does not matter. Changes to disks,
  folders and rules apply after a restart, and the window offers a **Restart now** button.
- Disks that are not NTFS (USB sticks, exFAT and FAT disks) are now indexed by walking their folders, and listed in
  the settings window. A stick that was seen before keeps its results while it is unplugged, and is walked again
  when it comes back. Optical drives and ReFS volumes are still left out.

### Changed
- The extra network folders of `roots.txt` still work; the same list is now also in the settings window (**More
  folders**).

## [0.15.1] - 2026-10-29

### Added
- Dates in words: `date:3d`, `2w`, `6m`, `1y` (the last N days, weeks, months, years); a weekday such as
  `date:monday` (the most recent one); `date:lastweek`, `lastmonth`, `lastyear` (the previous calendar week, month,
  year). Italian works too: `date:oggi`, `ieri`, `3g`, `2s`, `lunedì`, `settimana-scorsa`, `mese-scorso`.

## [0.15.0] - 2026-10-28

### Added
- Action menu on a result: press `→` (at the end of the text), `Ctrl+O`, or right-click. It offers open, show in
  folder, copy and cut (`Ctrl+C` / `Ctrl+X`: the file goes on the clipboard as Explorer puts it, so `Ctrl+V` in a
  folder pastes or moves it), copy path (`Ctrl+Shift+C`), open with, open terminal here (`Ctrl+T`, Windows Terminal
  or else a console) and properties (`Alt+Enter`). The shortcuts work with the menu closed too; with text selected in
  the search box, `Ctrl+C` and `Ctrl+X` still copy the text.
- Preview pane (`Alt+P`, remembered): the bar widens and shows the start of a text file (first 64 KB), or the
  picture Explorer shows in Large icons view (thumbnails of photos, videos, PDFs; a big icon otherwise). It is read
  only once the selection rests for 80 ms, so moving through the list stays as fast as before.
- Pins: `Ctrl+P` (or **Pin to top** in the action menu) pins a file, folder or app. Pinned items are listed
  first, with a 📌, before you type anything and whenever they match your search. `Ctrl+P` again unpins.

## [0.14.3] - 2026-10-28

### Added
- Folder sizes in the results: each folder shows the total of the files below it, kept current from the
  USN journal. Adding them up at load costs about 0.3 s per 1.6 million entries, in the background; searches are
  unchanged.

### Changed
- Updates for the portable exe: it now replaces itself with the new portable exe (verified by SHA-256, no
  permission prompt) and restarts, instead of running the installer into Program Files. A portable 0.14.2 still
  updates to this version through the installer, because the old update code runs; from 0.14.3 on it stays portable.

## [0.14.2] - 2026-10-28

### Added
- File sizes are shown at the right of each result (folders and files of unknown size show nothing).

### Changed
- Searches are faster again: entries are kept in one flat list that is split across the cores, instead of a hash
  map that had to be listed first. On 1.6 million entries a text search now takes about 35–60 ms (was 60–90 ms),
  `date:` and `size:` about 10–15 ms.
- The index cache is no longer rewritten every minute while files change. Changes are appended to a small log
  next to the cache and replayed at startup; the full cache is rewritten only when the log passes 16 MB, every
  six hours, or after a rescan. Less disk writing on a busy machine.

## [0.14.1] - 2026-10-28

### Changed
- Searches run on several cores: with 1.6 million files, a text search dropped from about 250 ms to 60–90 ms.
- Saving the index cache no longer pauses searches (it used to hold the index locked for a second or two, once a
  minute while files were changing).

## [0.14.0] - 2026-10-28

### Added
- `size:` filter: `size:>1gb`, `size:<10mb`, `size:10mb..1gb`, `size:empty` (units kb, mb, gb, tb; `>=` and `<=`
  also work). It matches files only and lists the biggest first when there are no words.

### Changed
- `date:` on local disks now uses the file's real modified time from its NTFS file record, not the time of its last
  USN journal record (which also moved on renames and permission changes). Edits are now tracked live too.
- The index stores each file's size. Local disks are rebuilt once after updating (the cache format changed), which
  reads one file record per file: about 11 seconds more for 1.2 million files, once per disk.

## [0.13.2] - 2026-10-28

### Added
- Updates from the tray menu: lisdir checks the GitHub releases a few seconds after start and then daily, and the
  menu shows **Update to X.Y.Z…** when a newer one exists. Clicking it downloads the installer, verifies its
  SHA-256 against `SHA256SUMS.txt`, and starts it (Windows asks for permission); lisdir restarts afterwards.
  Nothing is installed without a click. The installer accepts `/START=1` for this silent run.
- `Shift+Del` on the recent list clears all recent items at once.

### Changed
- Network folders with no cached index now show results while the first walk is still running, instead of only
  after it finishes.
- The app list is re-read from the Start menu when the bar opens (at most once a minute), so newly installed
  apps appear without restarting lisdir.

## [0.13.1] - 2026-10-28

### Fixed
- The selection no longer jumps back to the first result while you scroll during indexing; the list refreshes
  in place and keeps the selected item.

## [0.13.0] - 2026-10-28

### Added
- Search syntax, combinable and typed as plain text: `ext:pdf;docx` / `*.pdf`, `-draft` and `-ext:tmp` to exclude,
  `"exact phrase"`, `folder:` / `file:`, `date:` (`today`, `yesterday`, `week`, `month`, `year`, or `2026`,
  `2026-07`, `2026-07-31`), category shortcuts `doc:` `pic:` `video:` `music:` `zip:`, and `app:` for apps only.
  See the README for the table.
- Narrow by folder with a trailing backslash: `docs\ report` finds "report" inside a folder whose name contains
  "docs".
- Recent items: open the bar and type nothing to see what you opened lately. `Del` removes the selected one.
- Results you open often rank a little higher. The boost is small and capped, so how well the name matches always
  counts most.
- The letters matched by your search are highlighted in each result.
- With filters and no words (for example `date:week file:`), the newest files come first.

### Changed
- Several words in a query now match in any order (each must match); before, the whole text was matched as one
  sequence of letters.
- The matcher is 3 to 4 times faster: about 0.3 s instead of 1 to 1.5 s for a query over 2 million names
  (synthetic benchmark). A test now guards against regressions.
- The bar always opens empty, which shows the recent items; it used to keep the last search selected.
- The index remembers each file's last-change time. On the first start after updating, each disk is read again
  once, because older index caches do not have it.

### Fixed
- A network folder that is offline (powered-off NAS, dropped VPN, unplugged drive) no longer loses its search
  results: a walk that failed on more than 2% of the folders, or could not read the root, is discarded, the old
  index and cache are kept, and lisdir retries after a minute instead of ten.

## [0.12.0] - 2026-10-28

### Added
- App launcher: Start menu and Microsoft Store apps are found by the same fuzzy search as files and ranked together
  with them (an app outranks a file with an equally good name match). Apps show their own icon and open with
  `Enter`. The apps' Start menu shortcuts are hidden from the file results so they are not listed twice.
- File-type icons in the result list, taken from Windows itself (the same icons Explorer shows), so videos,
  photos, documents, archives, folders and programs are recognisable at a glance. Icons are looked up by file
  type without opening the file, so they are instant even on network shares; programs and shortcuts show their
  own icon.

### Changed
- The footer of the search bar no longer overlaps the last result when its text wraps to two lines.

## [0.11.0] - 2026-10-28

### Added
- Italian translation of the search bar, tray menu, installer, error messages and documentation
  (`README.it.md`). The language follows the Windows display language; set `LISDIR_LANG=it` or `en` to override.
- Release notes are bilingual (English and Italian).

## [0.10.0] - 2026-10-28

First release.

### Added
- Instant file and folder search over local NTFS disks, built by reading the MFT directly.
- Live index updates by following the NTFS USN journal, plus an on-disk index cache for fast startups.
- Network folder search: mapped drives and UNC paths (listed in `%LOCALAPPDATA%\lisdir\roots.txt`) are indexed
  by a parallel directory walk and refreshed every 10 minutes.
- Fuzzy matching with scoring that favours word starts and consecutive letters.
- Search bar on **Alt+Space**, with keyboard navigation (`Enter` opens, `Ctrl+Enter` reveals in Explorer,
  `Esc` closes).
- System-wide type-to-search: typing in an Explorer folder or on the desktop opens the bar with those letters.
- Tray icon with menu: type-to-search toggle, start with Windows, network folders, index service install, quit.
- Privilege split: the UI runs as a normal user and talks over a user-only named pipe to a windowless elevated
  helper, started at logon by a scheduled task.
- Windows installer (Inno Setup) and a portable executable, built and published by CI on version tags.
- Version number shown in the tray menu, tray tooltip, search bar and file properties.

[Unreleased]: https://github.com/enricoottavoita/lisdir/compare/v0.16.0...HEAD
[0.16.0]: https://github.com/enricoottavoita/lisdir/compare/v0.15.1...v0.16.0
[0.15.1]: https://github.com/enricoottavoita/lisdir/compare/v0.15.0...v0.15.1
[0.15.0]: https://github.com/enricoottavoita/lisdir/compare/v0.14.3...v0.15.0
[0.14.3]: https://github.com/enricoottavoita/lisdir/compare/v0.14.2...v0.14.3
[0.14.2]: https://github.com/enricoottavoita/lisdir/compare/v0.14.1...v0.14.2
[0.14.1]: https://github.com/enricoottavoita/lisdir/compare/v0.14.0...v0.14.1
[0.14.0]: https://github.com/enricoottavoita/lisdir/compare/v0.13.2...v0.14.0
[0.13.2]: https://github.com/enricoottavoita/lisdir/compare/v0.13.1...v0.13.2
[0.13.1]: https://github.com/enricoottavoita/lisdir/compare/v0.13.0...v0.13.1
[0.13.0]: https://github.com/enricoottavoita/lisdir/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/enricoottavoita/lisdir/compare/v0.11.0...v0.12.0
[0.11.0]: https://github.com/enricoottavoita/lisdir/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/enricoottavoita/lisdir/releases/tag/v0.10.0
