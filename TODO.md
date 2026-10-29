# TODO

Planned work, in priority order. Tick items as they ship and move them into [CHANGELOG.md](CHANGELOG.md).
Guiding rules: the bar must stay fast (a keystroke should never feel slower than the last version), the UI stays
unelevated, and every feature ships with a test or a measured check.

## Done
- [x] Live NTFS index from the MFT, followed through the USN journal, cached on disk (0.10)
- [x] Network folders indexed by a parallel walk; elevated helper with an unelevated UI (0.10)
- [x] Type-to-search in Explorer and on the desktop; installer; tag-driven releases (0.10)
- [x] English and Italian (0.11)
- [x] App launcher: Start menu and Store apps ranked together with files (0.12)
- [x] File-type icons in results (0.12)
- [x] Keep results when a network folder is unreachable (0.13)
- [x] Search syntax: `ext:`, `-exclude`, `"phrase"`, `folder:`/`file:`, `date:`, category keywords, `app:` (0.13)
- [x] Narrow by folder with a trailing `\` (0.13)
- [x] Ranking: name match first, modest boost for what you open often (0.13)
- [x] Recent items before you type (`Del` removes one); matched letters highlighted (0.13)
- [x] Latency budget test and a manual benchmark for millions of names (0.13)
- [x] Update check and one-click update from the tray menu, verified by SHA-256 (0.13.2)
- [x] Network results shown while the first walk is still running; app list refreshed when the bar opens;
      `Shift+Del` clears all recent items (0.13.2)
- [x] `size:` filter and exact modified times from the NTFS file records (0.14)
- [x] Search on several cores; the cache is saved without pausing searches (0.14.1)
- [x] File sizes shown in the results; changes appended to a log instead of rewriting the cache; entries in one
      flat list: a text search over 1.6 million entries takes 35–60 ms (0.14.2)
- [x] Folder sizes (sum of their files, kept current); the portable exe updates itself in place (0.14.3)
- [x] Action menu with copy / cut / copy path / open with / terminal / properties; preview pane (`Alt+P`);
      pinned results (`Ctrl+P`) (0.15)
- [x] Dates in words: `date:3d`, `date:monday`, `date:lastweek`, and the Italian forms (`date:ieri`, `date:lunedì`)
      (0.15.1)
- [x] Settings window: shortcut (plain character keys such as `Space` alone are refused), language, themes, tray
      icon, daily update check, Start with Windows, Type to search (0.16)
- [x] Choose which disks and extra folders are indexed (with a folder chooser, 0.16.2); **do not index** and
      **hide from results** rules (`node_modules`, `.git`, caches) (0.16)
- [x] Index non-NTFS local disks (USB sticks, exFAT/FAT) by walking them; results are kept while the drive is
      unplugged (0.16)

## 0.17 — search window
- [ ] Persistent search window that stays open to compare files
- [ ] Sortable columns including size and date modified
- [ ] Multi-select: copy, cut, delete, copy paths, drag several files at once
- [ ] Filter presets (Documents, Pictures, Last 7 days) that show their result counts before you apply them;
      Ctrl+click combines several
- [ ] Switch between the quick bar and the window with double-tap `Ctrl`, keeping the query

## 0.18 — diagnostics
- [ ] **"Why can't I find this?"** — give a path or file name and explain: not on an indexed disk, excluded,
      unsupported file system, helper not running, network folder not yet walked, and how to fix it
- [ ] About page: item counts, per-volume status, last walk times, and a small built-in speed test
- [ ] Tell the user when settings cannot be saved (the settings window shows the error; other places do not yet)
- [ ] Measure the memory used by the UI and the helper with 1.6 million entries and state it in the README

## Later
- [ ] Double-tap `Ctrl` as a shortcut (needs the low-level keyboard hook)
- [ ] Cloud-synced folders (OneDrive, Google Drive, Dropbox) as ordinary indexed folders
- [ ] Quick Save & Open: in Open/Save dialogs, jump to a recent folder, a searched folder, the folder you click in
      Explorer, or the last folder opened in a file manager; adapters for non-standard dialogs. Aim for
      near-instant popup
- [ ] Web search with keywords (`g query`, `w query`, `yt query`) and user-defined keywords
- [ ] Commands: run system commands or custom commands (also from the current folder) from the bar
- [ ] Mini calculator in the bar (`12*(3+4)`)
- [ ] Command line: `lisdir --search "query"` prints results through the existing pipe (scripts and agents)
- [ ] Optional local search API for scripts and AI tools, off by default, bound to localhost with a token
- [ ] Read-only MCP server (`lisdir --mcp`, over stdio) built on the command line search, so AI assistants can find
      files; search only, never open or change anything
- [ ] Community files: `SECURITY.md` (how to report a vulnerability) and `CODE_OF_CONDUCT.md`
- [ ] Clipboard history search and browser history search
- [ ] If search ever feels slow again: keep lowercase names in one byte buffer (no pointer per name) and/or a
      3-letter index (today 35–60 ms over 1.6 million entries, so not needed yet)
- [ ] Integration with third-party file managers
- [ ] Network folders: change detection instead of periodic re-walk
- [ ] Packaging: winget and Scoop manifests
- [ ] Code-signed executables (removes the SmartScreen warning)
- [ ] Windows on ARM build

## Not planned in this repo
- Built-in full-text search inside file contents (a different product category; an external content-search tool
  could be wired in through a keyword later)
- Pinyin and Korean initial-consonant matching (contributions welcome)
- Paid tiers: the project stays fully free and open source
