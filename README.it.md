# lisdir

[English](README.md) · **Italiano**

Una barra di ricerca e lanciatore di app veloce e open source per Windows. Premi **Alt+Spazio** (oppure inizia a
scrivere in una cartella di Esplora risorse o sul Desktop) e trova file, cartelle e app del computer mentre scrivi.

Scritto in Rust con interfaccia [Tauri](https://tauri.app). Licenza MIT.

## Funzioni

- **Ricerca istantanea su milioni di file.** I dischi NTFS locali sono indicizzati leggendo direttamente la MFT,
  senza scorrere le cartelle, molto più veloce di una scansione delle cartelle. Misurato su 1,6 milioni di voci
  (due dischi): una ricerca di testo richiede circa 35–60 ms; i filtri come `date:` e `size:` circa 10–15 ms.
- **Sempre aggiornato.** L'indice segue il journal USN di NTFS (controllato ogni secondo): file creati, rinominati,
  spostati o eliminati compaiono entro un paio di secondi, e così le modifiche (data e dimensione si aggiornano alla
  chiusura del file). L'indice è salvato su disco, quindi a ogni riavvio recupera solo ciò che è cambiato; nel frattempo
  le modifiche vengono aggiunte a un piccolo registro, senza riscrivere tutta la cache.
- **Cartelle di rete e altri dischi.** Unità mappate, percorsi UNC (condivisioni SMB) e dischi locali non NTFS
  (chiavette USB, exFAT, FAT) sono indicizzati con una scansione parallela e aggiornati ogni 10 minuti. Una chiavetta
  mantiene i suoi risultati mentre è scollegata. Alla primissima scansione i risultati compaiono mentre è in corso.
- **Digita per cercare.** Con una cartella di Esplora risorse o il Desktop in primo piano, scrivere delle lettere
  apre la barra con quelle lettere già inserite (si disattiva dal menu dell'icona o dalle impostazioni).
- **Ricerca approssimata.** `rdme` trova `README.md`; inizi di parola e lettere consecutive
  valgono di più. Più parole si trovano in qualsiasi ordine e le lettere trovate sono evidenziate.
- **Sintassi di ricerca.** Restringi con `ext:pdf`, `date:week`, `size:>1gb`, `-bozza`, `"frase esatta"`, `docs\ relazione`: vedi sotto.
- **Elementi recenti.** Apri la barra senza scrivere per vedere i preferiti fissati e poi cosa hai aperto di recente
  (`Canc` ne rimuove uno, `Maiusc+Canc` svuota l'elenco).
  Ciò che apri spesso sale un poco, ma conta sempre di più quanto il nome corrisponde.
- **Lanciatore di app.** Le app del menu Start e dello Store compaiono insieme ai file: scrivi `calc`, premi `Invio`.
- **Icone dei file e dimensioni.** I risultati mostrano le stesse icone di Esplora risorse, così video, foto,
  documenti e archivi si distinguono subito, e a destra la dimensione: per una cartella, il totale dei file che
  contiene, aggiornato quando cambiano.
- **Tutto da tastiera.** `Invio` apre, `Ctrl+Invio` mostra nella cartella, `Esc` chiude, `Ctrl+,` apre le impostazioni.
- **Azioni su un risultato.** `→`, `Ctrl+O` o il clic destro aprono un menu: copia o taglia il file (`Ctrl+C` /
  `Ctrl+X`, poi incollalo in Esplora risorse), copia il percorso (`Ctrl+Maiusc+C`), apri con, apri un terminale lì
  (`Ctrl+T`), proprietà (`Alt+Invio`). Le scorciatoie funzionano anche a menu chiuso.
- **Anteprima e preferiti fissati.** `Alt+P` mostra un riquadro di anteprima: l'inizio di un file di testo, o una
  miniatura. `Ctrl+P` fissa un risultato in cima, così compare sempre per primo.
- **Impostazioni.** La scorciatoia che apre la barra, la lingua, il tema dei colori (Windows, Scuro, Chiaro, Notte,
  Carta), l'avvio con Windows e cosa viene indicizzato: quali dischi, altre cartelle (con il selettore di cartelle) e
  regole per ciò che va lasciato fuori dall'indice o nascosto dai risultati.
- **Aggiornamenti.** Il menu dell'icona propone la nuova versione quando viene pubblicata e la installa dopo il tuo clic,
  dopo aver verificato lo SHA-256 (una copia portatile si aggiorna da sola, restando portatile). Questo controllo è
  l'unica richiesta internet che lisdir fa, e si può spegnere nelle impostazioni.
- **Interfaccia senza privilegi di amministratore.** La barra gira come utente normale; solo un piccolo
  programma in background è elevato.
- **Italiano e inglese.** La lingua segue quella di Windows, oppure si sceglie nelle impostazioni.

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
| `date:oggi`, `ieri`, `settimana`, `mese`, `anno` | come `today`, `yesterday`, `week`, `month`, `year` |
| `date:3g`, `2s`, `6m`, `1a` (o `3d`, `2w`, `1y`) | modificati negli ultimi 3 giorni / 2 settimane / 6 mesi / 1 anno |
| `date:lunedì` (o `lun`, `monday`) | modificati l'ultimo lunedì (oggi, se è lunedì) |
| `date:settimana-scorsa`, `mese-scorso`, `anno-scorso` | modificati nella settimana (lun–dom) / mese / anno precedente |
| `size:>1gb`, `size:<10mb`, `size:10mb..1gb`, `size:empty` | dimensione del file (`>` `>=` `<` `<=`, un intervallo, o vuoto); unità kb mb gb tb |
| `app:` | solo app |

Le parole chiave sono in inglese, qualunque sia la lingua dell'interfaccia (le date si possono scrivere anche in italiano). `date:` e `size:` usano la data di
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

**Cosa viene indicizzato.** Nella finestra delle impostazioni puoi togliere la spunta a un disco, aggiungere altre
cartelle da scansionare (con il pulsante **Aggiungi cartella…** scegli la cartella dal selettore di Windows, oppure
scrivi un percorso per riga, ad esempio `\\nas\condivisione`) e scrivere delle regole:
**Non indicizzare** lascia fuori dall'indice i nomi e le cartelle che corrispondono, con tutto quello che contengono
(risparmia memoria); **Nascondi dai risultati** li lascia indicizzati ma non li mostra mai. Una regola è un nome o un
modello (`node_modules`, `*.tmp`, `~$*`; `*` è un testo qualsiasi, `?` un carattere) oppure il percorso di una cartella
(`C:\Users\me\Cache`); maiuscole e minuscole non contano. Le modifiche a dischi, cartelle e regole valgono dopo un
riavvio, e la finestra ha il pulsante **Riavvia ora**. Rimuovere o cambiare una regola di non indicizzazione ricostruisce
una volta l'indice dei dischi locali.

I dischi locali che non sono NTFS (chiavette USB, dischi exFAT e FAT) vengono indicizzati scorrendo le cartelle, e una
chiavetta già vista mantiene i suoi risultati mentre è scollegata.

**Impostazioni** (menu dell'icona → Impostazioni…, `Ctrl+,` nella barra, oppure `lisdir.exe --settings`): la
scorciatoia che apre la barra (serve Ctrl, Alt o Win, oppure un tasto funzione; una già usata da un altro programma
viene rifiutata), la lingua, il tema dei colori, il nascondere l'icona, il controllo giornaliero degli aggiornamenti,
l'avvio con Windows, «digita per cercare» e cosa viene indicizzato (qui sopra). Sono salvate in
`%LOCALAPPDATA%\lisdir\settings.txt`, righe `chiave=valore`.

**Aggiornamenti.** lisdir controlla le release su GitHub pochi secondi dopo l'avvio e poi ogni giorno; se ce n'è una
nuova, il menu dell'icona propone «Aggiorna a X.Y.Z…» (oppure scegli «Controlla aggiornamenti…» quando vuoi). Scarica
l'installer, ne verifica lo SHA-256 con il file `SHA256SUMS.txt` pubblicato e lo avvia; Windows chiede il permesso e
lisdir si riapre alla fine. Non installa nulla senza un tuo clic, e il controllo giornaliero si spegne nelle impostazioni. Una copia portatile resta portatile: scarica il
nuovo exe portatile, lo verifica allo stesso modo, lo mette al posto del vecchio file e si riavvia (nessuna richiesta
di permesso; la cartella deve essere scrivibile).

## Compilare

Servono Windows 10/11 (64 bit), la toolchain [Rust](https://rustup.rs) (MSVC), gli strumenti di compilazione di Visual
Studio (carico di lavoro C++) e WebView2 (già presente su Windows 11).

```
cd src-tauri
cargo test
cargo build --release
```

## Limiti

- I dischi locali si leggono dalla MFT solo se sono NTFS; gli altri file system locali (exFAT, FAT) vengono scansionati
  cartella per cartella, e le modifiche compaiono al prossimo aggiornamento. I volumi ReFS e le unità ottiche non sono
  indicizzati.
- Le cartelle di rete vengono riscansionate, non osservate: le modifiche compaiono al prossimo aggiornamento.
- Una cartella di rete alla primissima scansione mostra risultati parziali finché non è finita (minuti per una
  condivisione molto grande); dagli avvii successivi usa la cache.
- Dimensioni e date dei dischi locali vengono dai record NTFS. Un file i cui dati stanno in un record di estensione
  (molto frammentato o con molti attributi) risulta di dimensione 0 e la dimensione non viene mostrata.
- "Digita per cercare" non controlla se c'è qualcosa di selezionato e non funziona sopra finestre elevate.
- I diritti di amministratore servono una volta, per configurare l'helper elevato; poi la barra non li richiede.
- La prima scansione di un disco locale legge un record per file per raccogliere dimensioni e date: circa 11 secondi in
  più per milione di file, una volta per disco (e di nuovo se cambia il formato della cache).
- Non si possono ancora ordinare i risultati per dimensione o data, e `size:` non trova le cartelle.
- L'anteprima legge il testo come UTF-8: le lettere accentate dei vecchi file ANSI appaiono come `�` e un file UTF-16 mostra la sua icona.
- Il menu azioni agisce su un risultato alla volta (più file insieme arriveranno con la finestra di ricerca).
- Le regole di nascondimento valgono per ciò che è indicizzato; le regole con un percorso indicano cartelle, non singoli
  file.
- Non ancora presenti: integrazione con i dialoghi Apri/Salva, ricerca nel contenuto. Il piano è in [TODO.md](TODO.md).

## Licenza

MIT, vedi [LICENSE](LICENSE). Progetto indipendente, non affiliato ad altri programmi di ricerca file.
