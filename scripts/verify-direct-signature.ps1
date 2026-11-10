param([Parameter(Mandatory=$true)][string]$FilePath)
$ErrorActionPreference = 'Stop'
$signature = Get-AuthenticodeSignature -LiteralPath $FilePath
if ($signature.Status -ne 'Valid') { throw 'AstreaAI runtime signature is invalid' }
