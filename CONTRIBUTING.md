# Contribuire a ScoreDraw

Puoi contribuire segnalando un problema, raccontando come usi ScoreDraw durante lo studio o un concerto, oppure proponendo una modifica al codice.

## Segnalare un problema

Scrivi a [info@emanuelecampilongo.com](mailto:info@emanuelecampilongo.com). Includi questi dettagli per aiutarci a riprodurre il problema.

- la versione di ScoreDraw da **? → Informazioni** e la versione di Windows
- i passaggi da seguire, quello che ti aspettavi e quello che è successo
- se sono rilevanti, il dispositivo, l’orientamento dello schermo, il livello di zoom e il tipo di input usato.

Quando il problema dipende da uno spartito, allega un PDF di esempio che puoi condividere, possibilmente con dati di prova.

## Proporre una modifica

Per una correzione circoscritta va bene aprire direttamente una pull request. Per una funzionalità nuova o un cambiamento ampio, scrivi prima via email per concordare il comportamento.

Compila ScoreDraw seguendo le [istruzioni per Windows](#compilare-su-windows), poi esegui i controlli locali.

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Nella pull request descrivi il problema, la soluzione e come l’hai verificata. Per modifiche all’interfaccia, aggiungi uno screenshot o spiega come hai provato il flusso con mouse, penna o touch.

## Compilare su Windows

Le istruzioni qui sotto usano Windows x64. Installa Rust stabile con [rustup](https://rust-lang.org/tools/install/) e il carico di lavoro **Sviluppo di applicazioni desktop con C++** di Visual Studio, con MSVC e il Windows SDK.

Apri PowerShell nella cartella dei sorgenti e lancia questi comandi.

```powershell
cargo build --release --locked
.\setup-pdfium.ps1
.\target\release\scoredraw.exe
```

`setup-pdfium.ps1` scarica la versione prevista di PDFium, verifica il checksum e copia la DLL e le relative licenze in `target\release`. Per un’altra cartella di output usa `-OutputDirectory`. Se hai già una toolchain isolata sotto `.toolchain`, puoi attivarla con `. .\dev.ps1`.

### Misurare il rendering

Per misurare apertura, anteprima e rasterizzazione delle prime pagine, indica una o più partiture e il percorso del report TSV.

```powershell
.\target\release\scoredraw.exe --misura "misure.tsv" "C:\Spartiti\Esempio.pdf"
```

### Preparare un pacchetto MSIX

Per generare un pacchetto MSIX di sviluppo usa MakeAppx del Windows SDK.

```powershell
.\package-msix.ps1 -BuildDirectory 'target\release'
```

Per compilare il pacchetto destinato allo Store, passa `-IdentityName`, `-Publisher` e `-PublisherDisplayName` con i valori assegnati in Partner Center.

Quando distribuisci un binario di ScoreDraw, rendi disponibile il sorgente corrispondente a quella versione secondo i termini della GPLv3.
