param([string]$Log = 'audit.log', [string]$Filter = '')
$ErrorActionPreference = 'Continue'
$env:LIBCLANG_PATH = 'C:/CodexLocal/build-tools/libclang/clang/native'
$arguments = @('test', '--offline', '--manifest-path', "$PSScriptRoot/Cargo.toml", '--target-dir', 'C:/CodexLocal/build/mcp-voice-host-security', '--test', 'audit')
if ($Filter) { $arguments += $Filter }
$arguments += @('--', '--nocapture')
& cargo @arguments 2>&1 | Tee-Object -FilePath "$PSScriptRoot/$Log" | Select-Object -Last 55
exit $LASTEXITCODE
