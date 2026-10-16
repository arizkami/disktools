# Builds the release exe and the Inno Setup installer into dist\.
# Usage: pwsh installer\build.ps1

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    $version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value

    $iscc = @(
        $env:ISCC,
        "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
        "$env:LOCALAPPDATA\InnoSetup\ISCC.exe"
    ) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
    if (-not $iscc) { throw 'ISCC.exe not found. Install Inno Setup 6 or set $env:ISCC.' }

    Write-Host "Building DiskTools $version (release)..."
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }

    Write-Host "Compiling installer with $iscc"
    & $iscc /Qp "/DAppVersion=$version" installer\disktools.iss
    if ($LASTEXITCODE -ne 0) { throw 'ISCC failed' }

    Get-Item "dist\disktools-$version-setup-x64.exe"
} finally {
    Pop-Location
}
