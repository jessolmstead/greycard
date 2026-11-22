# Check the Authenticode signatures SignPath put on our Windows files
# and, given the files as they went to it, that the signature is all
# it changed.
#
#     powershell -File scripts/windows-signature.ps1 -Policy <slug> -Signed <file or dir> [-Unsigned <file or dir>]
#
# The policy is the SignPath signing policy the files went through.
# Under release-signing each signature has to be valid, chained to a
# root Windows trusts, and timestamped, so it outlives the certificate.
# Under test-signing the certificate is SignPath's test one, which no
# Windows trusts, so the signature only has to be there and match the
# file it is on.
#
# -Unsigned is what went to SignPath: a directory holding the same
# names as -Signed, or a single file. Signing a PE file rewrites its
# checksum and its certificate table entry and appends the signature;
# every other byte has to be the one we built.

[CmdletBinding()]
param(
    [Parameter(Mandatory)][ValidateSet('release-signing', 'test-signing')][string]$Policy,
    [Parameter(Mandatory)][string]$Signed,
    [string]$Unsigned
)

$ErrorActionPreference = 'Stop'

function Fail([string]$what) { throw "windows-signature.ps1: $what" }

# A directory's files by their paths inside it, or a file by its name.
function Get-Names([string]$root) {
    if (Test-Path -LiteralPath $root -PathType Container) {
        @(Get-ChildItem -LiteralPath $root -File -Recurse |
            ForEach-Object { $_.FullName.Substring($root.Length).TrimStart('\') } |
            Sort-Object)
    } else {
        @((Get-Item -LiteralPath $root).Name)
    }
}

function Get-File([string]$root, [string]$name) {
    if (Test-Path -LiteralPath $root -PathType Container) { Join-Path $root $name } else { $root }
}

function Assert-OnlySigned([string]$before, [string]$after, [string]$name) {
    $a = [System.IO.File]::ReadAllBytes($before)
    $b = [System.IO.File]::ReadAllBytes($after)
    if ($b.Length -le $a.Length) { Fail "$name is no longer than it was unsigned" }
    # The optional header follows the PE signature and the COFF header;
    # its checksum is at 64, and the certificate table is the fifth
    # data directory, at 128 in a PE32 header and 144 in a PE32+ one.
    $header = [BitConverter]::ToInt32($a, 0x3C) + 24
    $table = $header + $(if ([BitConverter]::ToUInt16($a, $header) -eq 0x20B) { 144 } else { 128 })
    $c = New-Object byte[] $a.Length
    [Array]::Copy($b, $c, $a.Length)
    foreach ($bytes in $a, $c) {
        [Array]::Clear($bytes, $header + 64, 4)
        [Array]::Clear($bytes, $table, 8)
    }
    $sha = [System.Security.Cryptography.SHA256]::Create()
    if ([BitConverter]::ToString($sha.ComputeHash($a)) -ne [BitConverter]::ToString($sha.ComputeHash($c))) {
        Fail "$name differs from what went to SignPath in more than its signature"
    }
}

$Signed = (Resolve-Path -LiteralPath $Signed).Path
$names = Get-Names $Signed
if ($names.Count -eq 0) { Fail "nothing in $Signed" }
if ($Unsigned) {
    $Unsigned = (Resolve-Path -LiteralPath $Unsigned).Path
    $sent = Get-Names $Unsigned
    if (($sent -join '|') -ne ($names -join '|')) {
        Fail "SignPath sent back [$($names -join ', ')] for [$($sent -join ', ')]"
    }
}

foreach ($name in $names) {
    $file = Get-File $Signed $name
    $s = Get-AuthenticodeSignature -LiteralPath $file
    $by = if ($s.SignerCertificate) { $s.SignerCertificate.Subject } else { 'nobody' }
    $at = if ($s.TimeStamperCertificate) { 'timestamped' } else { 'no timestamp' }
    Write-Host "  ${name}: $($s.Status), signed by $by, $at"
    if (-not $s.SignerCertificate) { Fail "$name is not signed" }
    if ($s.Status -eq 'HashMismatch') { Fail "${name}'s signature does not match its contents" }
    if ($Policy -eq 'release-signing') {
        if ($s.Status -ne 'Valid') { Fail "${name}: $($s.StatusMessage)" }
        if (-not $s.TimeStamperCertificate) { Fail "${name}'s signature has no timestamp" }
    }
    if ($Unsigned) { Assert-OnlySigned (Get-File $Unsigned $name) $file $name }
}
