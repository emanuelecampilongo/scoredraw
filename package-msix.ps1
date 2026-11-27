param(
    [string]$BuildDirectory = (Join-Path $PSScriptRoot 'target\release'),
    [string]$MakeAppx,
    [string]$IdentityName = 'ScoreDraw.Development',
    [string]$Publisher = 'CN=ScoreDraw Development',
    [string]$PublisherDisplayName = 'ScoreDraw',
    [version]$Version = '0.4.16.0',
    [string]$OutputDirectory = (Join-Path $PSScriptRoot 'target\msix')
)

$ErrorActionPreference = 'Stop'
if ($Version.Revision -ne 0) { throw 'Per il Microsoft Store usa una versione che termina con .0.' }
if (-not $MakeAppx) {
    $command = Get-Command makeappx.exe -ErrorAction SilentlyContinue
    if ($command) { $MakeAppx = $command.Source }
    else {
        $bin = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
        if (Test-Path -LiteralPath $bin) {
            $MakeAppx = Get-ChildItem -LiteralPath $bin -Filter makeappx.exe -Recurse |
                Where-Object { $_.Directory.Name -eq 'x64' } |
                Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
        }
    }
}
if (-not $MakeAppx) { throw 'MakeAppx non trovato. Indica il percorso dello strumento con -MakeAppx.' }

$build = (Resolve-Path -LiteralPath $BuildDirectory).ProviderPath
$files = @('scoredraw.exe', 'pdfium.dll', 'PDFium-LICENSE')
foreach ($file in $files) {
    if (-not (Test-Path -LiteralPath (Join-Path $build $file) -PathType Leaf)) {
        throw "Manca $file nella cartella della build. Esegui la build e setup-pdfium.ps1."
    }
}
$pdfiumLicenses = Join-Path $build 'PDFium-licenses'
if (-not (Test-Path -LiteralPath $pdfiumLicenses -PathType Container) -or
    -not (Get-ChildItem -LiteralPath $pdfiumLicenses -File)) {
    throw 'Mancano le licenze delle dipendenze di PDFium. Esegui setup-pdfium.ps1 nella cartella della build.'
}
$exe = [IO.File]::ReadAllBytes((Join-Path $build 'scoredraw.exe'))
$peHeader = [BitConverter]::ToInt32($exe, 60)
$architecture = switch ([BitConverter]::ToUInt16($exe, $peHeader + 4)) {
    34404 { 'x64' }
    43620 { 'arm64' }
    332 { 'x86' }
    default { throw 'Architettura del file eseguibile non riconosciuta.' }
}

$output = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory)
$stage = Join-Path $output ([Guid]::NewGuid().ToString('N'))
$assets = Join-Path $stage 'Assets'
New-Item -ItemType Directory -Path $assets -Force | Out-Null
foreach ($file in $files) { Copy-Item -LiteralPath (Join-Path $build $file) -Destination $stage }
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'LICENSE') -Destination $stage
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'THIRD_PARTY_NOTICES.md') -Destination $stage
Copy-Item -LiteralPath $pdfiumLicenses -Destination $stage -Recurse
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'assets\scoredraw.ico') -Destination $assets

Add-Type -AssemblyName System.Drawing
foreach ($asset in @(
    @('Square150', 150, 'scoredraw'),
    @('Square44', 44, 'scoredraw'),
    @('StoreLogo', 50, 'scoredraw'),
    @('Pdf', 44, 'scoredraw-pdf'),
    @('Pdf.scale-200', 88, 'scoredraw-pdf'),
    @('Pdf.scale-400', 176, 'scoredraw-pdf')
)) {
    $size = [int]$asset[1]
    $source = [Drawing.Image]::FromFile((Join-Path $PSScriptRoot ('assets\' + $asset[2] + '.png')))
    $bitmap = [Drawing.Bitmap]::new($size, $size)
    $canvas = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $canvas.CompositingMode = [Drawing.Drawing2D.CompositingMode]::SourceCopy
        $canvas.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $canvas.DrawImage($source, 0, 0, $size, $size)
        $bitmap.Save((Join-Path $assets ($asset[0] + '.png')), [Drawing.Imaging.ImageFormat]::Png)
    } finally {
        $canvas.Dispose()
        $bitmap.Dispose()
        $source.Dispose()
    }
}

[xml]$manifest = @'
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
         xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
         xmlns:uap3="http://schemas.microsoft.com/appx/manifest/uap/windows10/3"
         xmlns:desktop7="http://schemas.microsoft.com/appx/manifest/desktop/windows10/7"
         xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
         IgnorableNamespaces="uap uap3 desktop7 rescap">
  <Identity Name="ScoreDraw.Development" Publisher="CN=ScoreDraw Development" Version="0.1.0.0"
            ProcessorArchitecture="x64" />
  <Properties>
    <DisplayName>ScoreDraw</DisplayName>
    <PublisherDisplayName>ScoreDraw</PublisherDisplayName>
    <Logo>Assets\StoreLogo.png</Logo>
  </Properties>
  <Resources><Resource Language="it-IT" /><Resource Language="en-US" /></Resources>
  <Dependencies>
    <TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.19041.0" MaxVersionTested="10.0.26100.0" />
  </Dependencies>
  <Applications>
    <Application Id="ScoreDraw" Executable="scoredraw.exe" EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements DisplayName="ScoreDraw" Description="Il lettore PDF pensato per i musicisti"
                          BackgroundColor="#1B1E18" Square150x150Logo="Assets\Square150.png"
                          Square44x44Logo="Assets\Square44.png" />
      <Extensions>
        <desktop7:Extension Category="windows.shortcut"
                            Executable="scoredraw.exe" EntryPoint="Windows.FullTrustApplication">
          <desktop7:Shortcut File="$(Desktop)\ScoreDraw.lnk" Icon="Assets\scoredraw.ico" />
        </desktop7:Extension>
        <uap:Extension Category="windows.fileTypeAssociation">
          <uap3:FileTypeAssociation Name="scoredrawpdf" Parameters="&quot;%1&quot;">
            <uap:DisplayName>Documento PDF · ScoreDraw</uap:DisplayName>
            <uap:Logo>Assets\Pdf.png</uap:Logo>
            <uap:SupportedFileTypes><uap:FileType>.pdf</uap:FileType></uap:SupportedFileTypes>
          </uap3:FileTypeAssociation>
        </uap:Extension>
      </Extensions>
    </Application>
  </Applications>
  <Capabilities>
    <rescap:Capability Name="runFullTrust" />
    <DeviceCapability Name="microphone" />
  </Capabilities>
</Package>
'@
$manifest.Package.Identity.SetAttribute('Name', $IdentityName)
$manifest.Package.Identity.SetAttribute('Publisher', $Publisher)
$manifest.Package.Identity.SetAttribute('Version', $Version.ToString())
$manifest.Package.Identity.SetAttribute('ProcessorArchitecture', $architecture)
$manifest.Package.Properties.PublisherDisplayName = $PublisherDisplayName
$manifest.Save((Join-Path $stage 'AppxManifest.xml'))

$package = Join-Path $output "ScoreDraw-$Version-$architecture.msix"
& $MakeAppx pack /d $stage /p $package /o
if ($LASTEXITCODE -ne 0) { throw "MakeAppx ha restituito il codice $LASTEXITCODE." }
Get-Item -LiteralPath $package
