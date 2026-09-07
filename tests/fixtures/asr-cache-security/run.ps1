$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
$manifest = Join-Path $PSScriptRoot 'Cargo.toml'
$sources = @('src/toolkit/asr/cache.rs', 'src/toolkit/asr/cache_tests.rs', 'src/toolkit/asr/cached.rs', 'src/toolkit/asr/cached_tests.rs', 'src/toolkit/asr/mod.rs', 'src/toolkit/asr/local.rs', 'src/toolkit/asr/openai.rs', 'src/toolkit/asr/database_media.rs', 'src/toolkit/audio/mod.rs', 'src/cli/asr.rs', 'src/cli/asr_database.rs', 'src/toolkit/files.rs')
Push-Location $repo
try {
    $before = @($sources | ForEach-Object { Get-FileHash -Algorithm SHA256 -LiteralPath $_ })
    $before | Format-List Algorithm, Hash, Path | Out-String | Tee-Object -FilePath (Join-Path $PSScriptRoot 'source-hashes.log')
    $env:LIBCLANG_PATH = 'C:\CodexLocal\build-tools\libclang\clang\native'
    foreach ($test in @(@('cache::tests', 'core'), @('security_tests', 'security-final'), @('cached::tests', 'cached'), @('adapter_tests', 'adapter'))) {
        $arguments = @('test', '--offline', '--manifest-path', $manifest, '--target-dir', 'C:\CodexLocal\build\asr-cache-security', $test[0], '--', '--nocapture')
        Write-Output ('cargo ' + ($arguments -join ' '))
        & cargo @arguments 2>&1 | Tee-Object -FilePath (Join-Path $PSScriptRoot ($test[1] + '.log'))
        if ($LASTEXITCODE -ne 0) { throw "Test failed: $($test[0]) (exit $LASTEXITCODE)" }
    }
    $after = @($sources | ForEach-Object { Get-FileHash -Algorithm SHA256 -LiteralPath $_ })
    if (Compare-Object ($before | ForEach-Object Hash) ($after | ForEach-Object Hash)) { throw 'Production sources changed during tests; rerun required' }
    Write-Output 'Reviewed source hashes unchanged across all four test groups.'
} finally {
    Pop-Location
}
