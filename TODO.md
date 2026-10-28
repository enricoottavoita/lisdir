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

## 0.12.x — fixes first
- [ ] **Keep results when a network folder is unreachable.** Today a failed walk replaces the index (and the cache)
      with an empty one, so a powered-off NAS loses all its results. Keep the old index when the walk fails or
      comes back empty; the same applies to removable drives that are unplugged
- [ ] Show network results while the first walk is still running (stream entries into the index)
- [ ] Refresh the app list when the Start menu changes, not only at startup

## 0.13 — search quality
- [ ] **Search syntax** (works as typed text, no UI needed):
      `ext:pdf;docx` or `*.pdf`, `date:today` / `date:week` / `date:2026-07`, `size:>1gb` / `size:10mb..1gb`,
      `"exact phrase"`, `-draft` and `-ext:tmp` to exclude, `folder:` / `file:`; all combinable
      (`ext:pdf date:week -draft contract`)
- [ ] Narrow by folder with a trailing `\` (`docs\ report` = "report" inside a folder matching "docs")
- [ ] Category keywords as shortcuts: `doc:`, `pic:`, `video:`, `music:`, `zip:`, `app:`
- [ ] **Ranking:** how well the name matches comes first; opening habits only give a modest boost (a file you
      opened once must not outrank a clearly better name match). Store usage counts locally
- [ ] Show recent files and folders before you type anything, with a way to hide or exclude entries
- [ ] Highlight the matched letters in each result
- [ ] Latency budget: a benchmark test that searches a few million synthetic entries and fails if it regresses

## 0.14 — actions and preview
- [ ] Action menu on a result (`Ctrl+O`, right arrow, right click) with shortcut hints in the footer
- [ ] Copy / cut a file to the clipboard (`Ctrl+C`, `Ctrl+X`), copy path, open with, open terminal here, properties
- [ ] Preview pane (`Alt+P`) that shows contents: text, images, thumbnails
- [ ] Pin files and folders so they always show up first

## 0.15 — settings and control
- [ ] Settings window: configurable hotkey (including double-tap `Ctrl`; reject plain character keys such as
      `Space` alone), language override, theme presets, hide the tray icon
- [ ] Choose which folders are indexed
- [ ] **Exclude from results** (still indexed, hidden from results) and **do not index** rules
      (`node_modules`, `.git`, caches)
- [ ] Index non-NTFS local disks (USB sticks, exFAT/FAT) with the directory walker; keep their results while the
      drive is unplugged
- [ ] Cloud-synced folders (OneDrive, Google Drive, Dropbox) as ordinary indexed folders

## 0.16 — search window
- [ ] Persistent search window that stays open to compare files
- [ ] Sortable columns including size and date modified; sizes shown in KB/MB/GB
- [ ] Multi-select: copy, cut, delete, copy paths, drag several files at once
- [ ] Filter presets (Documents, Pictures, Last 7 days) that show their result counts before you apply them;
      Ctrl+click combines several
- [ ] Switch between the quick bar and the window with double-tap `Ctrl`, keeping the query

## 0.17 — diagnostics
- [ ] **"Why can't I find this?"** — give a path or file name and explain: not on an indexed disk, excluded,
      unsupported file system, helper not running, network folder not yet walked, and how to fix it
- [ ] About page: item counts, per-volume status, last walk times, and a small built-in speed test
- [ ] Tell the user when settings cannot be saved

## Later
- [ ] Quick Save & Open: in Open/Save dialogs, jump to a recent folder, a searched folder, the folder you click in
      Explorer, or the last folder opened in a file manager; adapters for non-standard dialogs. Aim for
      near-instant popup
- [ ] Web search with keywords (`g query`, `w query`, `yt query`) and user-defined keywords
- [ ] Commands: run system commands or custom commands (also from the current folder) from the bar
- [ ] Mini calculator in the bar (`12*(3+4)`)
- [ ] Command line: `lisdir --search "query"` prints results through the existing pipe (scripts and agents)
- [ ] Optional local search API for scripts and AI tools, off by default, bound to localhost with a token
- [ ] Clipboard history search and browser history search
- [ ] Integration with third-party file managers
- [ ] Network folders: change detection instead of periodic re-walk
- [ ] Packaging and updates: winget and Scoop manifests, in-app update check
- [ ] Code-signed executables (removes the SmartScreen warning)
- [ ] Windows on ARM build

## Not planned in this repo
- Built-in full-text search inside file contents (a different product category; an external content-search tool
  could be wired in through a keyword later)
- Pinyin and Korean initial-consonant matching (contributions welcome)
- Paid tiers: the project stays fully free and open source
