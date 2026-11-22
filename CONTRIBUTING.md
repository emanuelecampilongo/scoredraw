# Contributing to ScoreDraw

Email [info@emanuelecampilongo.com](mailto:info@emanuelecampilongo.com) to report a bug, suggest a feature, or propose a code change. Email is the only channel for receiving and discussing changes.

For a bug report, include your ScoreDraw version from **? → About**, your Windows version, and the steps to reproduce the issue. Explain what you expected and what happened. When relevant, mention your device, screen orientation, zoom level, and whether you used a pen, touchscreen, trackpad, or mouse. If the issue depends on a specific score, attach a sample PDF you have permission to share.

For a proposed change, describe the problem or use case and the behavior you would like. If you already have a patch, you can attach it to your email. For changes to the interface, include a screenshot or describe how you tested the interaction.

## Build on Windows

These instructions target Windows x64. Install stable Rust through [rustup](https://rust-lang.org/tools/install/) and the **Desktop development with C++** workload in Visual Studio, including MSVC and the Windows SDK.

Open PowerShell in the source directory and run the following commands.

```powershell
cargo build --release --locked
.\setup-pdfium.ps1
.\target\release\scoredraw.exe
```

`setup-pdfium.ps1` downloads the pinned PDFium release, verifies its checksum, and copies the DLL and its license notices to `target\release`. Use `-OutputDirectory` to choose another build directory. If you already have an isolated toolchain under `.toolchain`, activate it with `. .\dev.ps1`.

Run the local checks before sending a patch.

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

### Measure PDF rendering

To measure document opening, preview generation, and rendering of the first pages, pass a report path followed by one or more PDFs.

```powershell
.\target\release\scoredraw.exe --misura "measurements.tsv" "C:\Scores\Example.pdf"
```

### Build an MSIX package

To create a development package, use MakeAppx from the Windows SDK. Run the PDFium setup script for the same build directory before packaging.

```powershell
.\package-msix.ps1 -BuildDirectory 'target\release'
```

For a Microsoft Store package, pass `-IdentityName`, `-Publisher`, and `-PublisherDisplayName` using the values from Partner Center.

If you distribute a ScoreDraw binary, make the corresponding source code available under the terms of the GPLv3.
