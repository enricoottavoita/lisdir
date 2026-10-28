# lisdir

[English](README.md) · **Italiano**

Una barra di ricerca e lanciatore di app veloce e open source per Windows. Premi **Alt+Spazio** (oppure inizia a
scrivere in una cartella di Esplora risorse o sul Desktop) e trova file, cartelle e app del computer mentre scrivi.

Scritto in Rust con interfaccia [Tauri](https://tauri.app). Licenza MIT.

## Funzioni

- **Ricerca istantanea su milioni di file.** I dischi NTFS locali sono indicizzati leggendo direttamente la MFT,
  senza scorrere le cartelle, molto più veloce di una scansione delle cartelle. Misurato su 1,6 milioni di voci (due dischi): una ricerca di
  testo richiede circa 35–60 ms; i filtri come `date:` e `size:` circa 10–15 ms.
- **Sempre aggiornato.** L'indice segue il journal USN di NTFS (controllato ogni secondo): file creati, rinominati,
  spostati o eliminati compaiono entro un paio di secondi. L'indice è salvato su disco, quindi a ogni riavvio
  recupera solo ciò che è cambiato.
- **Cartelle di rete.** Unità mappate e percorsi UNC (condivisioni SMB) sono indicizzati con una scansione
  parallela e aggiornati ogni 10 minuti.
- **Digita per cercare.** Con una cartella di Esplora risorse o il Desktop in primo piano, scrivere delle lettere
  apre la barra con quelle lettere già inserite (si disattiva dal menu dell'icona).
- **Ricerca approssimata.** `rdme` trova `README.md`; inizi di parola e lettere consecutive
  valgono di più. Più parole si trovano in qualsiasi ordine e le lettere trovate sono evidenziate.
- **Sintassi di ricerca.** Restringi con `ext:pdf`, `date:week`, `size:>1gb`, `-bozza`, `"frase esatta"`, `docs\ relazione`: vedi sotto.
- **Elementi recenti.** Apri la barra senza scrivere per vedere cosa hai aperto di recente (`Canc` ne rimuove uno).
  Ciò che apri spesso sale un poco, ma conta sempre di più quanto il nome corrisponde.
- **Lanciatore di app.** Le app del menu Start e dello Store compaiono insieme ai file: scrivi `calc`, premi `Invio`.
- **Icone dei file.** I risultati mostrano le stesse icone di Esplora risorse, così video, foto, documenti e
  archivi si distinguono subito.
- **Tutto da tastiera.** `Invio` apre, `Ctrl+Invio` mostra nella cartella, `Esc` chiude.
- **Aggiornamenti.** Il menu dell'icona propone la nuova versione quando viene pubblicata e la installa dopo il tuo clic,
  dopo aver verificato lo SHA-256. Questo controllo è l'unica richiesta internet che lisdir fa.
- **Interfaccia senza privilegi di amministratore.** La barra gira come utente normale; solo un piccolo
  programma in background è elevato.
- **Italiano e inglese.** La lingua segue quella di Windows.

## Sintassi di ricerca

Le parole semplici cercano nei nomi dei file; il resto restringe i risultati. Si può combinare tutto.

| Scrivi | Significato |
|---|---|
| `relazione 2024` | nomi che contengono entrambe le parole, in qualsiasi ordine (approssimata) |
| `ext:pdf;docx` oppure `*.pdf` | solo queste estensioni |
| `-bozza` / `-ext:tmp` | il nome non deve contenere `bozza` / non questa estensione |
| `"relazione annuale"` | la frase esatta nel nome |
| `docs\ relazione` | "relazione" dentro una cartella il cui nome contiene "docs" |
| `folder:` / `file:` | solo cartelle / solo file |
| `doc:` `pic:` `video:` `music:` `zip:` | un'intera categoria di tipi di file |
| `date:today`, `yesterday`, `week`, `month`, `year` | modificati oggi / ieri / negli ultimi 7, 30, 365 giorni |
| `date:2026`, `date:2026-07`, `date:2026-07-31` | modificati in quell'anno / mese / giorno |
| `size:>1gb`, `size:<10mb`, `size:10mb..1gb`, `size:empty` | dimensione del file (`>` `>=` `<` `<=`, un intervallo, o vuoto); unità kb mb gb tb |
| `app:` | solo app |

Le parole chiave sono in inglese, qualunque sia la lingua dell'interfaccia. `date:` e `size:` usano la data di
modifica e la dimensione del file, lette dal record NTFS (per le cartelle di rete, dall'elenco della cartella). Un
filtro `size:` trova solo file. Parole e filtri si combinano: `relazione ext:pdf date:month -bozza`. Con soli filtri
e nessuna parola, i file più recenti vengono per primi; con `size:`, i più grandi.

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

**Aggiornamenti.** lisdir controlla le release su GitHub pochi secondi dopo l'avvio e poi ogni giorno; se ce n'è una
nuova, il menu dell'icona propone «Aggiorna a X.Y.Z…» (oppure scegli «Controlla aggiornamenti…» quando vuoi). Scarica
l'installer, ne verifica lo SHA-256 con il file `SHA256SUMS.txt` pubblicato e lo avvia; Windows chiede il permesso e
lisdir si riapre alla fine. Non installa nulla senza un tuo clic. L'aggiornamento esegue sempre l'installer, quindi
una copia avviata dall'exe portatile finisce installata in Program Files.

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
- Una cartella di rete alla primissima scansione mostra risultati parziali finché non è finita (minuti per una
  condivisione molto grande); dagli avvii successivi usa la cache.
- Dimensioni e date dei dischi locali vengono dai record NTFS. Un file i cui dati stanno in un record di estensione
  (molto frammentato o con molti attributi) risulta di dimensione 0 e la dimensione non viene mostrata.
- "Digita per cercare" non controlla se c'è qualcosa di selezionato e non funziona sopra finestre elevate.
- I diritti di amministratore servono una volta, per configurare l'helper elevato; poi la barra non li richiede.
- La prima scansione di un disco locale legge un record per file per raccogliere dimensioni e date: circa 11 secondi in
  più per milione di file, una volta per disco (e di nuovo se cambia il formato della cache).
- Le cartelle non mostrano la dimensione, e non si possono ancora ordinare i risultati per dimensione o data.
- Non ancora presenti: menu azioni e anteprima, finestra delle impostazioni con scorciatoia configurabile,
  integrazione con i dialoghi Apri/Salva, ricerca nel contenuto. Il piano è in [TODO.md](TODO.md).

## Licenza

MIT, vedi [LICENSE](LICENSE). Progetto indipendente, non affiliato ad altri programmi di ricerca file.
