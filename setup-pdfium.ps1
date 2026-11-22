param(
    [ValidateSet('x64', 'arm64', 'x86')]
    [string]$Architecture = 'x64',
    [string]$OutputDirectory = (Join-Path $PSScriptRoot 'target\release')
)

$ErrorActionPreference = 'Stop'
$pdfiumVersion = '8125'
$archiveName = "pdfium-win-$Architecture.tgz"
$checksums = @{
    x64 = '9a964261760cff52570aabf038ab346d3a5034dd8b30d0d5fe8861235d356b8b'
    arm64 = 'd394ba61991fb53f4d87f70988035f337d9e73f2bd2c3c1b966df8c43fcabb05'
    x86 = '5aa1eb3e7de0589534245a181cb51def90d1dc76e987aea4be524a8cb3db1069'
}
$cache = Join-Path $PSScriptRoot "target\pdfium\$pdfiumVersion\$Architecture"
$archive = Join-Path $cache $archiveName
$url = "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F$pdfiumVersion/$archiveName"
New-Item -ItemType Directory -Path $cache, $OutputDirectory -Force | Out-Null
if (-not (Test-Path -LiteralPath $archive) -or (Get-FileHash -LiteralPath $archive).Hash -ne $checksums[$Architecture]) {
    Invoke-WebRequest -Uri $url -OutFile $archive
}
if ((Get-FileHash -LiteralPath $archive).Hash -ne $checksums[$Architecture]) {
    throw "Il checksum di $archiveName non corrisponde alla release PDFium."
}

$entries = @(tar.exe -tf $archive)
if ($LASTEXITCODE -ne 0) { throw 'Non è stato possibile leggere il pacchetto PDFium.' }
$members = @($entries | Where-Object { $_ -match '^(\./)?(bin/pdfium\.dll|LICEN[CS]E[^/]*|NOTICE[^/]*)$' })
$notices = @($entries | Where-Object { $_ -match '^(\./)?licenses/[A-Za-z0-9_.-]+\.[A-Za-z0-9]+$' })
if (-not ($members -match 'bin/pdfium\.dll$')) { throw 'pdfium.dll manca nel pacchetto.' }
if ($notices.Count -eq 0) { throw 'Gli avvisi di terze parti mancano nel pacchetto PDFium.' }
tar.exe -xf $archive -C $cache @members @notices
if ($LASTEXITCODE -ne 0) { throw 'Non è stato possibile estrarre PDFium.' }
Copy-Item -LiteralPath (Join-Path $cache 'bin\pdfium.dll') -Destination $OutputDirectory
foreach ($member in ($members -notmatch 'bin/pdfium\.dll$')) {
    $name = Split-Path $member -Leaf
    Copy-Item -LiteralPath (Join-Path $cache $member) -Destination (Join-Path $OutputDirectory "PDFium-$name")
}
$licenseDirectory = Join-Path $OutputDirectory 'PDFium-licenses'
New-Item -ItemType Directory -Path $licenseDirectory -Force | Out-Null
foreach ($notice in $notices) {
    Copy-Item -LiteralPath (Join-Path $cache $notice) -Destination $licenseDirectory
}
