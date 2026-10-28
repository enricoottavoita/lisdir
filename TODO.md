# TODO

Planned work, roughly in order. Tick items as they ship and move them into [CHANGELOG.md](CHANGELOG.md).

## 0.12 — launcher and ranking
- [x] App launcher: Start menu and Store apps searched together with files and ranked together
- [ ] Rank by habit: items you open often (and exact matches) float to the top
- [ ] Narrow by folder with a trailing `\` (`docs\ report` = "report" inside a folder matching "docs")
- [ ] Filters by file type and date (also usable as prefixes, e.g. `ext:pdf`, `folder:`)
- [ ] Highlight the matched letters in each result
- [x] File-type icons in results

## 0.13 — actions and preview
- [ ] Action menu on a result (`Ctrl+O`, right arrow, right click): copy, copy path, open with, open terminal here, properties
- [ ] Preview a result without opening it (`Alt+P`)
- [ ] Favourite folders menu for quick access

## 0.14 — settings
- [ ] Settings window: configurable hotkey (including double-tap `Ctrl`), theme, language override
- [ ] Choose which folders are indexed; exclusion rules (`node_modules`, `.git`, caches)
- [ ] Index non-NTFS local disks (USB sticks, exFAT/FAT) with the directory walker
- [ ] Cloud-synced folders (OneDrive, Google Drive, Dropbox) as ordinary indexed folders

## 0.15 — search window
- [ ] Persistent search window that stays open: sortable columns, filters, compare similar files
- [ ] Switch between the quick bar and the search window with double-tap `Ctrl`, keeping the query

## Later
- [ ] Web search with keywords (`g query`, `w query`, `yt query`) and user-defined keywords
- [ ] Commands: run system commands or custom commands (also from the current folder) from the bar
- [ ] Quick Save & Open: in Open/Save dialogs, jump to a recent folder, a searched folder, or the folder you click in Explorer; adapters for non-standard dialogs
- [ ] Mini calculator in the bar (`12*(3+4)`)
- [ ] Integration with third-party file managers
- [ ] Network folders: change detection instead of periodic re-walk
- [ ] Code-signed executables (removes the SmartScreen warning)

## Not planned
- Full-text search inside file contents (a different product category)
- Pinyin / Chinese-specific matching
- Paid tiers: the project stays fully free and open source
