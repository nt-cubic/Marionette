# gzip a file with .NET (build helper for the embedded-engine variant).
# PowerShell 5.1 compatible; usage: gzip-file.ps1 <in> <out.gz>
param(
    [Parameter(Mandatory = $true)][string]$In,
    [Parameter(Mandatory = $true)][string]$Out
)
$ErrorActionPreference = 'Stop'
$src = [IO.File]::OpenRead($In)
try {
    $dst = [IO.File]::Create($Out)
    try {
        $gz = New-Object System.IO.Compression.GZipStream($dst, [System.IO.Compression.CompressionLevel]::Optimal)
        try { $src.CopyTo($gz) } finally { $gz.Dispose() }
    } finally { $dst.Dispose() }
} finally { $src.Dispose() }
Write-Output ("[gzip] {0} -> {1} ({2:N1} MB)" -f $In, $Out, ((Get-Item $Out).Length / 1MB))
