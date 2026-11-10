# Installer for Conduit on Windows (PowerShell 5.1+).
#
#   irm https://raw.githubusercontent.com/resoul/conduit/main/install.ps1 | iex
#
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

function Fail([string]$Message) {
    Write-Host "Error: $Message" -ForegroundColor Red
    throw $Message
}

$repo = if ($env:CONDUIT_REPO) { $env:CONDUIT_REPO } else { 'resoul/conduit' }
$version = if ($env:CONDUIT_VERSION) { $env:CONDUIT_VERSION } else { 'latest' }
$installDir = if ($env:CONDUIT_INSTALL_DIR) { $env:CONDUIT_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'conduit\bin' }

$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
if ($arch -ne 'X64' -and $arch -ne 'Arm64') {
    Fail "Architecture $arch is not supported (x64 build available)"
}
$target = 'x86_64-pc-windows-msvc'
$asset = "conduit-$target.zip"

if ($env:CONDUIT_BASE_URL) {
    $base = $env:CONDUIT_BASE_URL
} elseif ($version -eq 'latest') {
    $base = "https://github.com/$repo/releases/latest/download"
} else {
    $base = "https://github.com/$repo/releases/download/$version"
}

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("conduit-install-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Write-Host "Platform: $target"
    Write-Host "Downloading $base/$asset..."
    $zip = Join-Path $tmp $asset
    Invoke-WebRequest -Uri "$base/$asset" -OutFile $zip -UseBasicParsing
    Invoke-WebRequest -Uri "$base/$asset.sha256" -OutFile "$zip.sha256" -UseBasicParsing

    $expected = ((Get-Content "$zip.sha256" -Raw).Trim() -split '\s+')[0].ToLower()
    $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
    if ($expected -ne $actual) {
        Fail "Checksum mismatch (expected $expected, got $actual); installation aborted"
    }
    Write-Host "Checksum verified successfully."

    Expand-Archive -Path $zip -DestinationPath $tmp -Force
    $bin = Join-Path $tmp 'conduit.exe'
    if (-not (Test-Path $bin)) {
        Fail "Archive does not contain conduit.exe"
    }

    if (-not (Test-Path $installDir)) {
        New-Item -ItemType Directory -Path $installDir -Force | Out-Null
    }
    Copy-Item -Path $bin -Destination (Join-Path $installDir 'conduit.exe') -Force
    Write-Host "Conduit installed successfully to $installDir\conduit.exe!" -ForegroundColor Green

    # Add to User PATH if not already present
    if (-not $env:CONDUIT_NO_MODIFY_PATH) {
        $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
        $parts = $userPath -split ';' | Where-Object { $_ -ne '' }
        if ($parts -notcontains $installDir) {
            $newPath = ($parts + $installDir) -join ';'
            [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
            $env:Path = "$env:Path;$installDir"
            Write-Host "Added $installDir to User PATH."
            Write-Host "Restart your terminal to use 'conduit' command."
        }
    }
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

