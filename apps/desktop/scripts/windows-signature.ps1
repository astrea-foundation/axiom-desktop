function Assert-AxiomSignature {
    param(
        [Parameter(Mandatory)][string] $FilePath,
        [Parameter(Mandatory)][string] $Publisher
    )
    $signature = Get-AuthenticodeSignature -LiteralPath $FilePath -ErrorAction Stop
    if ($signature.Status -ne 'Valid') {
        throw "Invalid Authenticode signature for ${FilePath}: $($signature.Status)"
    }
    $name = $signature.SignerCertificate.GetNameInfo([Security.Cryptography.X509Certificates.X509NameType]::SimpleName, $false)
    if ($name -cne $Publisher) { throw "Unexpected publisher for ${FilePath}: $name" }
    if ($null -eq $signature.TimeStamperCertificate) {
        throw "Missing trusted timestamp for $FilePath"
    }
    Write-Host "Verified timestamped signature: $FilePath ($name)"
}
