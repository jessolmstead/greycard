# Compile the Windows installer from what scripts/package.sh staged.
#
#     powershell -File scripts/windows-setup.ps1 [-Version <version>] [-Iscc <path>] [-SignedUninstallerDir <dir>]
#
# Run after package.sh on Windows. It compiles
# packaging/windows/greycard.iss over the staging directory
# package.sh left beside the zip, target/dist/greycard-<version>-<arch>-windows,
# so the setup and the zip carry the same files, and writes
# target/dist/greycard-<version>-<arch>-windows-setup.exe.
#
# The version is the workspace's, as package.sh reads it; an argument
# has to agree with it, for the reason package.sh gives. The compiler
# is -Iscc, else $env:ISCC, else ISCC.exe on the Path, else Inno Setup
# 7's default install directories.
#
# -SignedUninstallerDir, a directory relative to the tree, turns on
# Inno's signed-uninstaller mode, for a package signed through SignPath
# (scripts/signpath.sh): the first compile writes the uninstaller there
# and stops, asking for it to be signed; once a signed copy is there
# under the same name, the next compile puts its signature on the
# uninstaller and on Setup's temporary copies of itself. The setup
# comes out unsigned either way, for SignPath to sign after. Without
# it, nothing is signed.

[CmdletBinding()]
param(
    [string]$Version,
    [string]$Iscc,
    [string]$SignedUninstallerDir
)

$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')

$crate = ((cargo metadata --format-version 1 --no-deps | ConvertFrom-Json).packages |
    Where-Object { $_.name -eq 'greycard-ui' }).version
if (-not $crate) { throw 'windows-setup.ps1: cargo metadata has no version for greycard-ui' }
if (-not $Version) { $Version = $crate }
if ($Version -ne $crate) {
    throw "windows-setup.ps1: asked for $Version, but the workspace is at $crate."
}

# The host's triple names the package, as in package.sh, and says
# which Windows the setup may install on.
$hostTriple = (rustc -vV | Select-String '^host: (.*)$').Matches[0].Groups[1].Value
$arch = $hostTriple.Split('-')[0]
$allowed = switch ($arch) {
    'x86_64' { 'x64compatible' }
    'aarch64' { 'arm64' }
    default { throw "windows-setup.ps1: no installer for host $hostTriple" }
}

$name = "greycard-$Version-$arch-windows"
$stage = Join-Path (Get-Location).Path "target\dist\$name"
if (-not (Test-Path (Join-Path $stage 'bin\greycard-ui.exe'))) {
    throw "windows-setup.ps1: $stage has no bin\greycard-ui.exe; run scripts/package.sh first."
}

if (-not $Iscc) { $Iscc = $env:ISCC }
if (-not $Iscc) {
    $found = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    if ($found) { $Iscc = $found.Source }
}
if (-not $Iscc) {
    $Iscc = @(
        "$env:LOCALAPPDATA\Programs\Inno Setup 7\ISCC.exe",
        "$env:ProgramFiles\Inno Setup 7\ISCC.exe"
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $Iscc) { throw 'windows-setup.ps1: no ISCC.exe; install Inno Setup 7 or pass -Iscc.' }

# The file version resource takes digits alone: 0.12.0-rc.1 is 0.12.0.
$numeric = ($Version -split '[-+]')[0]

$arguments = @(
    "--define=Version=$Version",
    "--define=NumericVersion=$numeric",
    "--define=Stage=$stage",
    "--define=Arch=$allowed",
    "--output-dir=$((Resolve-Path 'target/dist').Path)",
    "--output-filename=$name-setup"
)
if ($SignedUninstallerDir) {
    $dir = [System.IO.Path]::GetFullPath((Join-Path (Get-Location).Path $SignedUninstallerDir))
    $arguments += "--define=SignedUninstallerDir=$dir"
}
$arguments += 'packaging/windows/greycard.iss'

& $Iscc --version
& $Iscc @arguments
if ($LASTEXITCODE -ne 0) { throw "windows-setup.ps1: ISCC failed with exit code $LASTEXITCODE" }
Write-Host "target/dist/$name-setup.exe"
