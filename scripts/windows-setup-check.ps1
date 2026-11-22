# Install the setup windows-setup.ps1 made, check every thing it puts
# on the machine, uninstall it, and check every thing is gone.
#
#     powershell -File scripts/windows-setup-check.ps1 -Setup <setup.exe> -Version <version> [-Signature <policy>]
#
# With -Signature, the SignPath signing policy the package went
# through, the two executables and the uninstaller as installed are
# checked for its signature (scripts/windows-signature.ps1): the
# uninstaller is signed nowhere but inside the setup, so this is the
# one place its signature can be seen.
#
# For a runner, which is thrown away after: it installs into the
# running user's profile. The user's Path gets an entry holding an
# unexpanded %USERPROFILE% first, so the check also sees the installer
# keep a variable it did not write, the trap install.ps1's header
# describes. Another application's offer goes on .cr3 first too, so
# the check sees the uninstaller take only our own value out of a key
# it shares.

[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Setup,
    [Parameter(Mandatory)][string]$Version,
    [ValidateSet('release-signing', 'test-signing')][string]$Signature
)

$ErrorActionPreference = 'Stop'

$app = Join-Path $env:LOCALAPPDATA 'Programs\greycard'
$link = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\greycard.lnk'
$classes = 'HKCU:\Software\Classes'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\greycard_is1'
$marker = '%USERPROFILE%\greycard-setup-check'
$neighbor = 'greycard-setup-check.other'

function Fail([string]$what) { throw "windows-setup-check.ps1: $what" }

# The raw extensions, read from register.cmd, so the setup's own copy
# of the list is checked against the one the zip registers.
$register = Join-Path $PSScriptRoot '..\packaging\windows\register.cmd'
$offered = if ((Get-Content -Raw $register) -match '(?m)^for %%E in \(([^)]*)\) do') {
    @($Matches[1] -split '\s+' | Where-Object { $_ })
} else { Fail "no list of raw extensions in $register" }

function Get-RawPath {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment')
    try {
        [string]$key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    } finally { $key.Close() }
}

function Get-PathEntries { @((Get-RawPath) -split ';' | Where-Object { $_ }) }

# An offer is a value whose data is nothing, so it is asked for by name.
function Test-Offer([string]$ext, [string]$progid) {
    $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\Classes\$ext\OpenWithProgids")
    if (-not $k) { return $false }
    try { $k.GetValueNames() -contains $progid } finally { $k.Close() }
}

function Get-Default([string]$key) {
    $k = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\Classes\$key")
    if (-not $k) { return $null }
    try { $k.GetValue('') } finally { $k.Close() }
}

function Run([string]$file, [string[]]$arguments) {
    $p = Start-Process -FilePath $file -ArgumentList $arguments -Wait -PassThru
    if ($p.ExitCode -ne 0) { Fail "$file exited with $($p.ExitCode)" }
}

# The two logs, printed when anything fails: the setup's and the
# uninstaller's own account of what they did.
$log = Join-Path ([System.IO.Path]::GetTempPath()) 'greycard-setup.log'
$unlog = Join-Path ([System.IO.Path]::GetTempPath()) 'greycard-uninstall.log'

# Neither starts a shell: Inno logs every program it runs by its path.
function Assert-NoShell([string]$file) {
    if (-not (Test-Path $file)) { Fail "no log at $file" }
    $ran = Select-String -Path $file -Pattern '\\(cmd|powershell|pwsh|reg)\.exe' -AllMatches
    if ($ran) { Fail "$file shows a shell run: $($ran.Line -join ' | ')" }
}

Remove-Item $log, $unlog -ErrorAction SilentlyContinue

try {
    # A Path entry with a variable in it, as a user's may have.
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    $key.SetValue('Path', ((@(Get-PathEntries) + $marker) -join ';'),
        [Microsoft.Win32.RegistryValueKind]::ExpandString)
    $key.Close()
    $before = Get-PathEntries

    # Another application's offer for .cr3, in the key ours joins.
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Software\Classes\.cr3\OpenWithProgids')
    $key.SetValue($neighbor, '', [Microsoft.Win32.RegistryValueKind]::String)
    $key.Close()

    Write-Host "installing $Setup"
    Run (Resolve-Path $Setup).Path @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$log`"")
    Assert-NoShell $log

    foreach ($f in 'greycard-ui.exe', 'greycard.exe', 'application-x-greycard-edit.ico',
        'LICENSE', 'THIRD-PARTY-NOTICES', 'README.md', 'unins000.exe') {
        if (-not (Test-Path (Join-Path $app $f))) { Fail "$app\$f is missing" }
    }
    # The registration is the setup's own; the zip's scripts stay out.
    foreach ($f in 'register.cmd', 'unregister.cmd') {
        if (Test-Path (Join-Path $app $f)) { Fail "$app\$f was installed" }
    }
    Get-ChildItem $app -File | ForEach-Object { Write-Host "  $($_.Name)" }
    if ($Signature) {
        foreach ($f in 'greycard-ui.exe', 'greycard.exe', 'unins000.exe') {
            & (Join-Path $PSScriptRoot 'windows-signature.ps1') -Policy $Signature -Signed (Join-Path $app $f)
        }
    }

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

    # The associations, value for value as register.cmd writes them.
    $exe = Join-Path $app 'greycard-ui.exe'
    $open = "`"$exe`" `"%1`""
    $want = [ordered]@{
        '.gcd'                                       = 'greycard.gcd'
        'greycard.gcd'                               = 'greycard edit sidecar'
        'greycard.gcd\DefaultIcon'                   = "`"$(Join-Path $app 'application-x-greycard-edit.ico')`""
        'greycard.gcd\shell\open\command'            = $open
        'greycard.photo'                             = 'greycard photo'
        'greycard.photo\DefaultIcon'                 = "`"$exe`",0"
        'greycard.photo\shell\open\command'          = $open
        'Applications\greycard-ui.exe\shell\open\command' = $open
    }
    foreach ($k in $want.Keys) {
        $got = Get-Default $k
        if ($got -ne $want[$k]) { Fail "$k holds [$got], not [$($want[$k])]" }
    }
    if ((Get-ItemProperty "$classes\Applications\greycard-ui.exe").FriendlyAppName -ne 'greycard') {
        Fail 'Applications\greycard-ui.exe has no FriendlyAppName greycard'
    }
    if (-not (Test-Offer '.gcd' 'greycard.gcd')) { Fail '.gcd\OpenWithProgids does not hold greycard.gcd' }
    foreach ($ext in $offered) {
        if (-not (Test-Offer $ext 'greycard.photo')) { Fail "$ext does not offer greycard.photo" }
    }
    if (-not (Test-Offer '.cr3' $neighbor)) { Fail "the install took $neighbor off .cr3" }

    $entry = Get-ItemProperty $uninstallKey
    if ($entry.DisplayVersion -ne $Version) { Fail "the uninstall entry says $($entry.DisplayVersion)" }
    Write-Host "  installed: Start menu, Path, associations ($($offered -join ' ')), and $($entry.DisplayName) $($entry.DisplayVersion) in Installed apps"

    Write-Host 'uninstalling'
    Run (Join-Path $app 'unins000.exe') @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$unlog`"")
    # The uninstaller's first copy waits for the one that does the work,
    # but give the directory a moment rather than trust that.
    for ($i = 0; $i -lt 30 -and (Test-Path $app); $i++) { Start-Sleep -Seconds 1 }
    Assert-NoShell $unlog

    if (Test-Path $app) { Fail "$app is still there: $(Get-ChildItem $app -Recurse | Out-String)" }
    if (Test-Path $link) { Fail "$link is still there" }
    $left = Get-PathEntries
    if (($left -join ';') -ne ($before -join ';')) { Fail "the Path is not as it was: $(Get-RawPath)" }
    if (Test-Path "$classes\.gcd") { Fail '.gcd is still registered' }
    foreach ($k in 'greycard.gcd', 'greycard.photo', 'Applications\greycard-ui.exe') {
        if (Test-Path "$classes\$k") { Fail "$k is still registered" }
    }
    foreach ($ext in $offered) {
        if (Test-Offer $ext 'greycard.photo') { Fail "$ext still offers greycard.photo" }
    }
    # Ours gone from the shared key, the other application's left.
    if (-not (Test-Offer '.cr3' $neighbor)) { Fail "the uninstall took $neighbor off .cr3" }
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
} finally {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Classes\.cr3\OpenWithProgids', $true)
    if ($key) {
        $key.DeleteValue($neighbor, $false)
        $empty = $key.ValueCount -eq 0 -and $key.SubKeyCount -eq 0
        $key.Close()
        if ($empty) {
            [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKey('Software\Classes\.cr3\OpenWithProgids', $false)
        }
    }
}
