# Changelog

All notable changes to lisdir are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

When you tag a release `vX.Y.Z`, the release workflow publishes the matching `## [X.Y.Z]` section below as the
release notes, and fails if that section is missing. Add changes under **Unreleased** as you go, then rename it
when releasing.

## [Unreleased]

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

[Unreleased]: https://github.com/enricoottavoita/lisdir/compare/v0.14.0...HEAD
[0.14.0]: https://github.com/enricoottavoita/lisdir/compare/v0.13.2...v0.14.0
[0.13.2]: https://github.com/enricoottavoita/lisdir/compare/v0.13.1...v0.13.2
[0.13.1]: https://github.com/enricoottavoita/lisdir/compare/v0.13.0...v0.13.1
[0.13.0]: https://github.com/enricoottavoita/lisdir/compare/v0.12.0...v0.13.0
[0.12.0]: https://github.com/enricoottavoita/lisdir/compare/v0.11.0...v0.12.0
[0.11.0]: https://github.com/enricoottavoita/lisdir/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/enricoottavoita/lisdir/releases/tag/v0.10.0
