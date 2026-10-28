# lisdir

[English](README.md) · **Italiano**

Una barra di ricerca file veloce e open source per Windows. Premi **Alt+Spazio** (oppure inizia a scrivere
in una cartella di Esplora risorse o sul Desktop) e trova qualsiasi file o cartella del computer mentre scrivi.

Scritto in Rust con interfaccia [Tauri](https://tauri.app). Licenza MIT.

## Funzioni

- **Ricerca istantanea su milioni di file.** I dischi NTFS locali sono indicizzati leggendo direttamente la MFT,
  senza scorrere le cartelle: l'indice di un disco grande è pronto in pochi secondi.
- **Sempre aggiornato.** L'indice segue il journal USN di NTFS: file creati, rinominati, spostati o eliminati
  compaiono entro circa un secondo. L'indice è salvato su disco, quindi gli avvii successivi sono quasi immediati.
- **Cartelle di rete.** Unità mappate e percorsi UNC (condivisioni SMB) sono indicizzati con una scansione
  parallela e aggiornati ogni 10 minuti.
- **Digita per cercare.** Con una cartella di Esplora risorse o il Desktop in primo piano, scrivere delle lettere
  apre la barra con quelle lettere già inserite (si disattiva dal menu dell'icona).
- **Ricerca approssimata.** `lsrysvc` trova `Listary.Service.exe`; inizi di parola e lettere consecutive
  valgono di più.
- **Tutto da tastiera.** `Invio` apre, `Ctrl+Invio` mostra nella cartella, `Esc` chiude.
- **Interfaccia senza privilegi di amministratore.** La barra gira come utente normale; solo un piccolo
  programma in background è elevato.
- **Italiano e inglese.** La lingua segue quella di Windows.

## Come funziona

Leggere la MFT di un disco richiede un accesso diretto al volume, quindi i diritti di amministratore. Invece di
avviare tutta l'app come amministratore (così ogni file aperto partirebbe con privilegi elevati), un helper
senza finestre tiene gli indici NTFS e risponde alle ricerche tramite una named pipe accessibile solo al tuo
utente. La barra indicizza da sola le cartelle di rete, perché solo la tua sessione ha le tue credenziali e le
tue unità mappate. Lo schema completo è nel [README in inglese](README.md#how-it-works).

## Installazione

**Installer (consigliato).** Scarica `lisdir-setup-<versione>.exe` dalla pagina delle
[Release](../../releases) ed eseguilo. Chiede i diritti di amministratore una sola volta: per installare in
Program Files e registrare l'attività pianificata che avvia l'helper elevato a ogni accesso. Poi premi
**Alt+Spazio**. Si disinstalla da Impostazioni di Windows, e questo rimuove anche l'attività.

**Portatile.** Esegui `lisdir-<versione>-portable.exe`, poi dal menu dell'icona scegli
**Installa servizio di indicizzazione (admin)…** e accetta la richiesta UAC, una volta sola.

Gli eseguibili non sono ancora firmati, quindi SmartScreen può mostrare un avviso: *Ulteriori informazioni →
Esegui comunque*.

Altre cartelle di rete si aggiungono in `%LOCALAPPDATA%\lisdir\roots.txt`, un percorso per riga (dal menu
dell'icona: **Cartelle di rete…**).

## Compilare

Servono Windows 10/11, la toolchain [Rust](https://rustup.rs) (MSVC), gli strumenti di compilazione di Visual
Studio (carico di lavoro C++) e WebView2 (già presente su Windows 11).

```
cd src-tauri
cargo test
cargo build --release
```

## Limiti

- Solo NTFS per i dischi locali (ReFS, exFAT e FAT non sono indicizzati).
- Le cartelle di rete vengono riscansionate, non osservate: le modifiche compaiono al prossimo aggiornamento.
- "Digita per cercare" non controlla se c'è qualcosa di selezionato e non funziona sopra finestre elevate.
- Non ancora presenti: evidenziazione delle lettere trovate, icone dei file, scorciatoia configurabile,
  integrazione con i dialoghi Apri/Salva, ricerca nel contenuto.

## Licenza

MIT, vedi [LICENSE](LICENSE). Progetto indipendente, non affiliato ad altri programmi di ricerca file.
