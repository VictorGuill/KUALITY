<#
.SYNOPSIS
    Builds the release exe and packages Kuality (docs/specs/distribution.md).

.DESCRIPTION
    Produces dist\Kuality-<version>-portable.zip (kuality.exe, the `portable` marker, LICENSE.txt,
    README.txt). With -Installer it also compiles installer\kuality.iss with Inno Setup's ISCC.exe
    (dist\Kuality-<version>-setup.exe) if Inno Setup 6 is installed.

.EXAMPLE
    .\scripts\package.ps1
    .\scripts\package.ps1 -Installer
    .\scripts\package.ps1 -SkipBuild     # package the existing target\release\kuality.exe
#>
[CmdletBinding()]
param(
    [switch]$Installer,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    if (-not $SkipBuild) {
        cargo build --release
        if ($LASTEXITCODE -ne 0) { throw "cargo build --release failed" }
    }
    $exe = Join-Path $root 'target\release\kuality.exe'
    if (-not (Test-Path $exe)) { throw "$exe not found" }

    # The version embedded by build.rs (from the workspace version).
    $version = (Get-Item $exe).VersionInfo.ProductVersion
    if (-not $version) { throw "kuality.exe has no version resource" }

    $dist = Join-Path $root 'dist'
    $name = "Kuality-$version-portable"
    $stage = Join-Path $dist $name
    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    New-Item -ItemType Directory -Force $stage | Out-Null

    Copy-Item $exe $stage
    # Empty marker: config and logs live next to the exe.
    New-Item -ItemType File (Join-Path $stage 'portable') | Out-Null
    Copy-Item (Join-Path $root 'LICENSE') (Join-Path $stage 'LICENSE.txt')
    (Get-Content -Raw (Join-Path $root 'installer\README-portable.txt')).Replace('{version}', $version) |
        Set-Content -NoNewline -Encoding utf8 (Join-Path $stage 'README.txt')

    $zip = Join-Path $dist "$name.zip"
    if (Test-Path $zip) { Remove-Item -Force $zip }
    Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
    Remove-Item -Recurse -Force $stage
    $size = [math]::Round((Get-Item $zip).Length / 1MB, 2)
    Write-Host "Portable: $zip ($size MB)"

    if ($Installer) {
        $iscc = @(
            (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source,
            "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
            "$env:ProgramFiles\Inno Setup 6\ISCC.exe",
            "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe"
        ) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
        if (-not $iscc) {
            Write-Warning "Inno Setup 6 (ISCC.exe) not found; installer not built."
        } else {
            & $iscc "/DAppVersion=$version" (Join-Path $root 'installer\kuality.iss')
            if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }
            Write-Host "Installer: $(Join-Path $dist "Kuality-$version-setup.exe")"
        }
    }
} finally {
    Pop-Location
}
