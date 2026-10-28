# lisdir

[English](README.md) · **Italiano**

Una barra di ricerca e lanciatore di app veloce e open source per Windows. Premi **Alt+Spazio** (oppure inizia a
scrivere in una cartella di Esplora risorse o sul Desktop) e trova file, cartelle e app del computer mentre scrivi.

Scritto in Rust con interfaccia [Tauri](https://tauri.app). Licenza MIT.

## Funzioni

- **Ricerca istantanea su milioni di file.** I dischi NTFS locali sono indicizzati leggendo direttamente la MFT,
  senza scorrere le cartelle, molto più veloce di una scansione delle cartelle.
- **Sempre aggiornato.** L'indice segue il journal USN di NTFS (controllato ogni secondo): file creati, rinominati,
  spostati o eliminati compaiono entro un paio di secondi. L'indice è salvato su disco, quindi a ogni riavvio
  recupera solo ciò che è cambiato.
- **Cartelle di rete.** Unità mappate e percorsi UNC (condivisioni SMB) sono indicizzati con una scansione
  parallela e aggiornati ogni 10 minuti.
- **Digita per cercare.** Con una cartella di Esplora risorse o il Desktop in primo piano, scrivere delle lettere
  apre la barra con quelle lettere già inserite (si disattiva dal menu dell'icona).
- **Ricerca approssimata.** `rdme` trova `README.md`; inizi di parola e lettere consecutive
  valgono di più.
- **Lanciatore di app.** Le app del menu Start e dello Store compaiono insieme ai file: scrivi `calc`, premi `Invio`.
- **Icone dei file.** I risultati mostrano le stesse icone di Esplora risorse, così video, foto, documenti e
  archivi si distinguono subito.
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

Servono Windows 10/11 (64 bit), la toolchain [Rust](https://rustup.rs) (MSVC), gli strumenti di compilazione di Visual
Studio (carico di lavoro C++) e WebView2 (già presente su Windows 11).

```
cd src-tauri
cargo test
cargo build --release
```

## Limiti

- Solo NTFS per i dischi locali (ReFS, exFAT e FAT non sono indicizzati).
- Le cartelle di rete vengono riscansionate, non osservate: le modifiche compaiono al prossimo aggiornamento.
- Una cartella di rete non dà risultati finché la prima scansione non è finita (minuti per una condivisione molto
  grande); dagli avvii successivi usa la cache.
- L'elenco delle app viene letto all'avvio di lisdir: un'app appena installata compare dopo un riavvio.
- "Digita per cercare" non controlla se c'è qualcosa di selezionato e non funziona sopra finestre elevate.
- I diritti di amministratore servono una volta, per configurare l'helper elevato; poi la barra non li richiede.
- Non ancora presenti: ordinamento in base a quanto usi i file, filtri di ricerca, evidenziazione delle lettere
  trovate, menu azioni e anteprima, finestra delle impostazioni con scorciatoia configurabile, integrazione con i
  dialoghi Apri/Salva, ricerca nel contenuto. Il piano è in [TODO.md](TODO.md).

## Licenza

MIT, vedi [LICENSE](LICENSE). Progetto indipendente, non affiliato ad altri programmi di ricerca file.
