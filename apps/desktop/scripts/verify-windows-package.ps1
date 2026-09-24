param(
    [Parameter(Mandatory)][string] $Unpacked,
    [Parameter(Mandatory)][ValidateSet('x64', 'arm64')][string] $Arch,
    [Parameter(Mandatory)][string] $Version,
    [string] $Installer,
    [string] $Publisher,
    [switch] $Combined
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. "$PSScriptRoot/windows-signature.ps1"
if ($Version -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') { throw 'A stable version is required' }
$Unpacked = (Resolve-Path -LiteralPath $Unpacked).Path
$app = Join-Path $Unpacked 'Axiom.exe'
$cli = Join-Path $Unpacked 'resources/bin/axiomcli.exe'
$expectedMachine = if ($Arch -eq 'arm64') { 0xAA64 } else { 0x8664 }
foreach ($file in $app, $cli) {
    $stream = [IO.File]::OpenRead($file)
    $reader = [IO.BinaryReader]::new($stream)
    try {
        if ($reader.ReadUInt16() -ne 0x5A4D) { throw "Not a PE executable: $file" }
        $stream.Position = 0x3C
        $offset = $reader.ReadInt32()
        if ($offset -lt 0 -or $offset + 6 -gt $stream.Length) { throw "Invalid PE offset: $file" }
        $stream.Position = $offset
        if ($reader.ReadUInt32() -ne 0x4550 -or $reader.ReadUInt16() -ne $expectedMachine) {
            throw "Incorrect PE architecture: $file"
        }
    } finally { $reader.Dispose() }
}
$versioned = @($app)
if ($Installer) {
    $Installer = (Resolve-Path -LiteralPath $Installer).Path
    $installerArch = if ($Combined) { 'universal' } else { $Arch }
    if ([IO.Path]::GetFileName($Installer) -cne "Axiom-$Version-win-$installerArch.exe") { throw 'Unexpected installer filename' }
    if (-not (Test-Path -LiteralPath "$Installer.blockmap")) { throw 'Missing installer blockmap' }
    $dist = Split-Path $Installer -Parent
    if (@(Get-ChildItem -LiteralPath $dist -File -Filter '*.exe').Count -ne 1 -or
        @(Get-ChildItem -LiteralPath $dist -File -Filter '*.blockmap').Count -ne 1) {
        throw 'Expected exactly one installer and one blockmap'
    }
    $versioned += $Installer
}
$parts = $Version.Split('.')
foreach ($file in $versioned) {
    $info = [Diagnostics.FileVersionInfo]::GetVersionInfo($file)
    if ($info.ProductMajorPart -ne [int]$parts[0] -or $info.ProductMinorPart -ne [int]$parts[1] -or
        $info.ProductBuildPart -ne [int]$parts[2] -or $info.ProductPrivatePart -ne 0) {
        throw "Incorrect product version: $file"
    }
}
if ($Publisher) {
    $native = @(Get-ChildItem -LiteralPath $Unpacked -Recurse -File | Where-Object { $_.Extension -in '.exe', '.dll', '.node', '.ps1' })
    foreach ($file in $native) { Assert-AxiomSignature -FilePath $file.FullName -Publisher $Publisher }
    if ($Installer) { Assert-AxiomSignature -FilePath $Installer -Publisher $Publisher }
}
