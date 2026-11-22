$scoreDrawToolchain = Join-Path $PSScriptRoot '.toolchain'
$env:RUSTUP_HOME = Join-Path $scoreDrawToolchain 'rustup'
$env:CARGO_HOME = Join-Path $scoreDrawToolchain 'cargo'
$scoreDrawCargoBin = Join-Path $env:CARGO_HOME 'bin'
if (-not (Test-Path -LiteralPath (Join-Path $scoreDrawCargoBin 'cargo.exe'))) {
    throw 'Toolchain locale assente. Consulta README.md per la configurazione di Rust.'
}
if ($scoreDrawCargoBin -notin ($env:Path -split ';')) {
    $env:Path = "$scoreDrawCargoBin;$env:Path"
}
$env:CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER = 'rust-lld'
