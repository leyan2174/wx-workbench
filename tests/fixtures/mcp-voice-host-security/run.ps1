param([string]$Log = '', [string]$Filter = '')
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
if (-not $Log) { $Log = Join-Path $root 'target/test-logs/mcp-voice-host-security/audit.log' }
$Log = [IO.Path]::GetFullPath($Log)
New-Item -ItemType Directory -Path ([IO.Path]::GetDirectoryName($Log)) -Force | Out-Null
$arguments = @('test', '--locked', '--offline', '--manifest-path', "$PSScriptRoot/Cargo.toml", '--target-dir', (Join-Path $root 'target/fixtures/mcp-voice-host-security'), '--test', 'audit')
if ($Filter) { $arguments += $Filter }
$arguments += @('--', '--nocapture')
Write-Output ('cargo ' + ($arguments -join ' '))
& cargo @arguments 2>&1 | Tee-Object -FilePath $Log
exit $LASTEXITCODE
