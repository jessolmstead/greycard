# Install the setup windows-setup.ps1 made, check every thing it puts
# on the machine, uninstall it, and check every thing is gone.
#
#     powershell -File scripts/windows-setup-check.ps1 -Setup <setup.exe> -Version <version>
#
# For a runner, which is thrown away after: it installs into the
# running user's profile. The user's Path gets an entry holding an
# unexpanded %USERPROFILE% first, so the check also sees the installer
# keep a variable it did not write, the trap install.ps1's header
# describes.

[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Setup,
    [Parameter(Mandatory)][string]$Version
)

$ErrorActionPreference = 'Stop'

$app = Join-Path $env:LOCALAPPDATA 'Programs\greycard'
$link = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\greycard.lnk'
$classes = 'HKCU:\Software\Classes'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\greycard_is1'
$marker = '%USERPROFILE%\greycard-setup-check'

function Fail([string]$what) { throw "windows-setup-check.ps1: $what" }

function Get-RawPath {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment')
    try {
        [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    } finally { $key.Close() }
}

function Get-PathEntries { @((Get-RawPath) -split ';' | Where-Object { $_ }) }

# The offer is a REG_NONE value with no data, so it is asked for by name.
function Test-Offer {
    $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Classes\.cr3\OpenWithProgids')
    if (-not $k) { return $false }
    try { $k.GetValueNames() -contains 'greycard.photo' } finally { $k.Close() }
}

function Run([string]$file, [string[]]$arguments) {
    $p = Start-Process -FilePath $file -ArgumentList $arguments -Wait -PassThru
    if ($p.ExitCode -ne 0) { Fail "$file exited with $($p.ExitCode)" }
}

# The two logs, printed when anything fails: the setup's and the
# uninstaller's own account of what they did.
$log = Join-Path ([System.IO.Path]::GetTempPath()) 'greycard-setup.log'
$unlog = Join-Path ([System.IO.Path]::GetTempPath()) 'greycard-uninstall.log'

Remove-Item $log, $unlog -ErrorAction SilentlyContinue

try {
    # A Path entry with a variable in it, as a user's may have.
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    $key.SetValue('Path', ((@(Get-PathEntries) + $marker) -join ';'),
        [Microsoft.Win32.RegistryValueKind]::ExpandString)
    $key.Close()
    $before = Get-PathEntries

    Write-Host "installing $Setup"
    Run (Resolve-Path $Setup).Path @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$log`"")

    foreach ($f in 'greycard-ui.exe', 'greycard.exe', 'application-x-greycard-edit.ico',
        'register.cmd', 'unregister.cmd', 'LICENSE', 'THIRD-PARTY-NOTICES', 'README.md',
        'unins000.exe') {
        if (-not (Test-Path (Join-Path $app $f))) { Fail "$app\$f is missing" }
    }
    Get-ChildItem $app -File | ForEach-Object { Write-Host "  $($_.Name)" }

    $said = & (Join-Path $app 'greycard.exe') --version
    if ($LASTEXITCODE -ne 0 -or "$said" -notmatch [regex]::Escape($Version)) {
        Fail "the installed greycard.exe --version said: $said"
    }
    Write-Host "  greycard --version: $said"

    if (-not (Test-Path $link)) { Fail "no $link" }
    $target = (New-Object -ComObject WScript.Shell).CreateShortcut($link).TargetPath
    if ($target -ne (Join-Path $app 'greycard-ui.exe')) { Fail "$link points at $target" }

    $after = Get-PathEntries
    if (-not ($after | Where-Object { $_.TrimEnd('\') -eq $app })) { Fail "$app is not on the Path" }
    if (-not ($after -contains $marker)) { Fail "the Path lost ${marker}: $(Get-RawPath)" }

    if ((Get-ItemProperty "$classes\.gcd").'(default)' -ne 'greycard.gcd') { Fail '.gcd is not greycard.gcd' }
    if (-not (Test-Offer)) { Fail '.cr3 does not offer greycard.photo' }
    foreach ($k in 'greycard.gcd', 'greycard.photo', 'Applications\greycard-ui.exe') {
        if (-not (Test-Path "$classes\$k\shell\open\command")) { Fail "$k is not registered" }
    }
    $progids = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Classes\.gcd\OpenWithProgids')
    if (-not $progids -or -not ($progids.GetValueNames() -contains 'greycard.gcd')) {
        Fail '.gcd\OpenWithProgids does not hold greycard.gcd'
    }
    $progids.Close()

    $entry = Get-ItemProperty $uninstallKey
    if ($entry.DisplayVersion -ne $Version) { Fail "the uninstall entry says $($entry.DisplayVersion)" }
    Write-Host "  installed: Start menu, Path, associations, and $($entry.DisplayName) $($entry.DisplayVersion) in Installed apps"

    Write-Host 'uninstalling'
    Run (Join-Path $app 'unins000.exe') @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$unlog`"")
    # The uninstaller's first copy waits for the one that does the work,
    # but give the directory a moment rather than trust that.
    for ($i = 0; $i -lt 30 -and (Test-Path $app); $i++) { Start-Sleep -Seconds 1 }

    if (Test-Path $app) { Fail "$app is still there: $(Get-ChildItem $app -Recurse | Out-String)" }
    if (Test-Path $link) { Fail "$link is still there" }
    $left = Get-PathEntries
    if (($left -join ';') -ne ($before -join ';')) { Fail "the Path is not as it was: $(Get-RawPath)" }
    if (Test-Path "$classes\.gcd") { Fail '.gcd is still registered' }
    foreach ($k in 'greycard.gcd', 'greycard.photo', 'Applications\greycard-ui.exe') {
        if (Test-Path "$classes\$k") { Fail "$k is still registered" }
    }
    if (Test-Offer) { Fail '.cr3 still offers greycard.photo' }
    if (Test-Path $uninstallKey) { Fail 'the uninstall entry is still there' }
    Write-Host '  removed: files, Start menu, Path entry, associations, and the uninstall entry'
} catch {
    foreach ($l in $log, $unlog) {
        if (Test-Path $l) {
            Write-Host "----- $l"
            Get-Content $l | Write-Host
        }
    }
    throw
}
