param([Parameter(Mandatory)][string] $FilePath,[Parameter(Mandatory)][string] $Version,[string] $Publisher,[switch] $RequireLicenses)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$FilePath = (Resolve-Path -LiteralPath $FilePath).Path
. "$PSScriptRoot/../../desktop/scripts/windows-signature.ps1"
if ($Publisher) { Assert-AxiomSignature -FilePath $FilePath -Publisher $Publisher }
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('axiom-setup-check-' + [Guid]::NewGuid().ToString('N') + '.json')
try {
    $process = Start-Process -FilePath $FilePath -ArgumentList @('--verify-build', '--metadata-output', "`"$temporary`"") -PassThru
    $null = $process.Handle
    if (-not $process.WaitForExit(30000)) { throw 'Setup self-check timed out' }
    $process.Refresh()
    if ($process.ExitCode -ne 0) { throw 'Setup self-check failed' }
    $metadata = Get-Content -LiteralPath $temporary -Raw | ConvertFrom-Json
    $expectedKeys = if ($env:AXIOM_UPDATE_PUBLIC_KEYS) { $env:AXIOM_UPDATE_PUBLIC_KEYS } else { '' }
    if ($metadata.version -cne $Version -or $metadata.trustedKeys -cne $expectedKeys) { throw 'Setup version or compiled update trust mismatch' }
    if ($Publisher -and $metadata.publisher -cne $Publisher) { throw 'Setup publisher mismatch' }
    if (($Publisher -or $RequireLicenses) -and -not $metadata.licensesIncluded) { throw 'Setup dependency notices are missing' }
    Write-Host "PASS setup $Version starts and builds its native controls on $([Runtime.InteropServices.RuntimeInformation]::OSArchitecture)"
} finally { Remove-Item -LiteralPath $temporary -Force -ErrorAction SilentlyContinue }
