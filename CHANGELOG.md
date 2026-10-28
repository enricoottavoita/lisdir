# Changelog

All notable changes to lisdir are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

When you tag a release `vX.Y.Z`, the release workflow publishes the matching `## [X.Y.Z]` section below as the
release notes, and fails if that section is missing. Add changes under **Unreleased** as you go, then rename it
when releasing.

## [Unreleased]

### Added
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

[Unreleased]: https://github.com/enricoottavoita/lisdir/compare/v0.11.0...HEAD
[0.11.0]: https://github.com/enricoottavoita/lisdir/compare/v0.10.0...v0.11.0
[0.10.0]: https://github.com/enricoottavoita/lisdir/releases/tag/v0.10.0
