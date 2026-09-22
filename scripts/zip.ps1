# Write a staged directory into a .zip, deterministically.
#
#     powershell -File scripts/zip.ps1 -Source <dir> -Destination <file> -Date <iso8601>
#
# scripts/package.sh calls this for the Windows archive, where the
# tarball the other two platforms get would be a file Windows 10's
# Explorer cannot open by double-click. The care taken here is the same
# care taken over there and for the same reason: nothing about who
# rolled the archive or when belongs inside it, so two builds of one
# commit come out byte for byte the same. Entries are written in sorted
# order rather than whatever order the filesystem hands back, their
# names use forward slashes, and every timestamp is the commit's date
# rather than the moment the file was copied.
#
# Compress-Archive is not used: it writes entries in enumeration order
# and stamps them with the file's own mtime, so its output differs from
# run to run.

[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Source,
    [Parameter(Mandatory)][string]$Destination,
    [Parameter(Mandatory)][string]$Date
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

$root = (Resolve-Path $Source).Path.TrimEnd('\')
$stamp = [DateTimeOffset]::Parse($Date).ToUniversalTime()

# A zip's own timestamps are MS-DOS, which cannot go below 1980; a
# repository with no commits hands package.sh 1970 and would throw here.
if ($stamp.Year -lt 1980) { $stamp = [DateTimeOffset]::Parse('1980-01-01T00:00:00Z') }

if (Test-Path $Destination) { Remove-Item $Destination -Force }
$zip = [System.IO.Compression.ZipFile]::Open($Destination, 'Create')
try {
    # Ordinal, not Sort-Object: the tarball's entry list is sorted
    # under LC_ALL=C, and PowerShell's sort is culture-aware even
    # with -CaseSensitive, so it would order greycard-ui.exe against
    # greycard.exe differently on a runner with another locale and
    # the archive would stop being reproducible.
    $files = [string[]] @(Get-ChildItem $root -Recurse -File |
        ForEach-Object { $_.FullName.Substring($root.Length + 1).Replace('\', '/') })
    [Array]::Sort($files, [StringComparer]::Ordinal)
    foreach ($name in $files) {
        # The entry is made, stamped and only then opened: in Create
        # mode a ZipArchiveEntry refuses a timestamp once its stream
        # has been written to, which is what CreateEntryFromFile does
        # in one step.
        $entry = $zip.CreateEntry($name, [System.IO.Compression.CompressionLevel]::Optimal)
        $entry.LastWriteTime = $stamp
        $source = Join-Path $root ($name -replace '/', '\')
        $out = $entry.Open()
        try {
            $bytes = [System.IO.File]::ReadAllBytes($source)
            $out.Write($bytes, 0, $bytes.Length)
        } finally { $out.Dispose() }
    }
} finally {
    $zip.Dispose()
}

Write-Host "$(@($files).Count) files"
