param(
    [Parameter(Mandatory)][string] $FilePath,
    [Parameter(Mandatory)][string] $Publisher
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. "$PSScriptRoot/windows-signature.ps1"
Assert-AxiomSignature -FilePath $FilePath -Publisher $Publisher
