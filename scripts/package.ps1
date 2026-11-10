# Package conduit.exe binary into a zip release archive with sha256 checksum.
#
#   scripts/package.ps1 <target-triple> [binary-path]
#
param(
    [Parameter(Mandatory=$true)][string]$Target,
    [string]$Bin
)
$ErrorActionPreference = 'Stop'

if (-not $Bin) {
    if (Test-Path "target\$Target\release\conduit.exe") {
        $Bin = "target\$Target\release\conduit.exe"
    } elseif (Test-Path "target\release\conduit.exe") {
        $Bin = "target\release\conduit.exe"
    } else {
        throw "conduit.exe not found. Run 'cargo build --release -p conduit' first."
    }
}

$out = "dist"
New-Item -ItemType Directory -Path $out -Force | Out-Null
$stage = Join-Path ([System.IO.Path]::GetTempPath()) ("conduit-pkg-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null

try {
    Copy-Item $Bin (Join-Path $stage 'conduit.exe')
    $archive = Join-Path $out "conduit-$Target.zip"
    Compress-Archive -Path (Join-Path $stage 'conduit.exe') -DestinationPath $archive -Force

    $hash = (Get-FileHash $archive -Algorithm SHA256).Hash.ToLower()
    $hashFile = "$archive.sha256"
    "$hash  conduit-$Target.zip" | Set-Content $hashFile -NoNewline

    Write-Host "Created release artifact: $archive"
    Write-Host (Get-Content $hashFile)
}
finally {
    Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
}

