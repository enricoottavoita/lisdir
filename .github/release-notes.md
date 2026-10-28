## Install

1. Download **lisdir-setup-&lt;version&gt;.exe** below and run it. It asks for administrator rights once, to
   install into Program Files and to register the small background helper that reads your disks.
2. Press **Alt+Space** to open the search bar.

The portable `lisdir-<version>-portable.exe` is the same program without the installer. Run it, then use the
tray icon menu → **Install index service (admin)…** once.

The executables are not code-signed yet, so Windows SmartScreen may warn you: choose *More info → Run anyway*.
Checksums are in `SHA256SUMS.txt`.

Already installed? Use the tray icon menu → **Check for updates…**; lisdir also checks by itself once a day.

## Installazione

1. Scarica **lisdir-setup-&lt;versione&gt;.exe** qui sotto ed eseguilo. Chiede i diritti di amministratore una
   volta sola, per installare in Program Files e registrare il piccolo programma in background che legge i dischi.
2. Premi **Alt+Spazio** per aprire la barra di ricerca.

La versione portatile `lisdir-<versione>-portable.exe` è lo stesso programma senza installer: eseguila e poi, dal
menu dell'icona, scegli **Installa servizio di indicizzazione (admin)…** una volta.

Gli eseguibili non sono ancora firmati, quindi SmartScreen può mostrare un avviso: *Ulteriori informazioni →
Esegui comunque*. I checksum sono in `SHA256SUMS.txt`.

Hai già installato lisdir? Dal menu dell'icona scegli **Controlla aggiornamenti…**; lisdir controlla comunque da
solo una volta al giorno.

## Notes / Note

- Local NTFS disks only; network folders (SMB) are indexed by a directory walk. / Solo dischi NTFS locali; le
  cartelle di rete (SMB) sono indicizzate con una scansione delle cartelle.
- To uninstall, use Windows Settings → Apps. This also removes the helper's scheduled task. / Per disinstallare
  usa Impostazioni → App: viene rimossa anche l'attività pianificata dell'helper.
