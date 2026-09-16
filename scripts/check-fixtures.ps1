param([switch]$Offline)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$targetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repoRoot 'target' }
if (-not [IO.Path]::IsPathRooted($targetRoot)) { $targetRoot = Join-Path $repoRoot $targetRoot }
$targetRoot = [IO.Path]::GetFullPath($targetRoot)
$ownerFile = Join-Path $targetRoot '.checkout-owner'
if (Test-Path -LiteralPath $ownerFile) {
    $owner = [IO.Path]::GetFullPath((Get-Content -LiteralPath $ownerFile -Raw).Trim())
    if ($owner -ne $repoRoot) { throw 'Build directory belongs to another checkout; choose a dedicated target.' }
} else {
    $insideCheckout = $targetRoot.StartsWith($repoRoot + [IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)
    if (-not $insideCheckout -and (Test-Path -LiteralPath $targetRoot) -and (Get-ChildItem -LiteralPath $targetRoot -Force | Select-Object -First 1)) {
        throw 'External build directory has no checkout owner; use a fresh target or the default checkout target.'
    }
    New-Item -ItemType Directory -Force -Path $targetRoot | Out-Null
    $ownerStream = [IO.File]::Open($ownerFile, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
    try {
        $ownerBytes = [Text.Encoding]::UTF8.GetBytes($repoRoot)
        $ownerStream.Write($ownerBytes, 0, $ownerBytes.Length)
    } finally {
        $ownerStream.Dispose()
    }
}
$logRoot = Join-Path $targetRoot 'quality-fixtures'
New-Item -ItemType Directory -Force -Path $logRoot | Out-Null
$manifests = @(Get-ChildItem -Path (Join-Path $repoRoot 'tests/fixtures/*/Cargo.toml') -File)
$manifests += Get-Item -LiteralPath (Join-Path $repoRoot 'src/crypto/test-harness/Cargo.toml')
$results = @()
$previousBinary = $env:CARGO_BIN_EXE_wx
$env:CARGO_BIN_EXE_wx = Join-Path $targetRoot 'x86_64-pc-windows-msvc/debug/wx.exe'

Push-Location $repoRoot
try {
    foreach ($manifest in $manifests) {
        $name = $manifest.Directory.Name
        # These three libraries intentionally expose named integration targets instead of libtest.
        # Keep this policy aligned with tests/support/CHECK_MATRIX.md.
        $targets = switch ($name) {
            'mcp-image-security' { @('--lib', '--bins', '--test', 'audit') }
            'mcp-readonly-security' { @('--lib', '--bins', '--test', 'security') }
            'wav-publish' { @('--lib', '--bins', '--test', 'publisher') }
            default { @('--all-targets') }
        }
        $cargoArgs = @('check', '--locked', '--target', 'x86_64-pc-windows-msvc', '--manifest-path', $manifest.FullName) + $targets
        if ($Offline) { $cargoArgs += '--offline' }
        if ($name -in @('mcp-history-compat', 'plan-selection')) { $cargoArgs += @('--features', 'runtime') }
        $logPath = Join-Path $logRoot "$name.log"
        Write-Host ('cargo ' + (($cargoArgs | ForEach-Object { '"' + $_ + '"' }) -join ' '))
        & cargo @cargoArgs *> $logPath
        $code = $LASTEXITCODE
        $results += [pscustomobject]@{ fixture = $name; exit_code = $code; log = $logPath }
        Write-Host "$name : exit $code; full log: $logPath"
    }
} finally {
    $env:CARGO_BIN_EXE_wx = $previousBinary
    Pop-Location
}
$results | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $logRoot 'summary.json') -Encoding utf8
if ($results.Where({ $_.exit_code -ne 0 }).Count -gt 0) { exit 1 }
