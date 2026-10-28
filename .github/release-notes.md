## Install

1. Download **lisdir-setup-&lt;version&gt;.exe** below and run it. It asks for administrator rights once, to
   install into Program Files and to register the small background helper that reads your disks.
2. Press **Alt+Space** to open the search bar.

The portable `lisdir-<version>-portable.exe` is the same program without the installer. Run it, then use the
tray icon menu → **Install index service (admin)…** once.

The executables are not code-signed yet, so Windows SmartScreen may warn you: choose *More info → Run anyway*.
Checksums are in `SHA256SUMS.txt`.

## Notes

- Local NTFS disks only; network folders (SMB) are indexed by a directory walk.
- To uninstall, use Windows Settings → Apps. This also removes the helper's scheduled task.
