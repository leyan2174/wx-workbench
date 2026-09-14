param([string]$TestFilter = '', [string]$TargetDir = '', [switch]$Check)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$mode = if ($Check) { 'check' } else { 'test' }
$arguments = @($mode, '--locked', '--manifest-path', (Join-Path $root 'Cargo.toml'), '--target', 'x86_64-pc-windows-msvc', '--bin', 'wx')
if ($TargetDir) { $arguments += @('--target-dir', $TargetDir) }
if (-not $Check) {
    if ($TestFilter) { $arguments += $TestFilter }
    $arguments += @('--', '--test-threads=1')
}
Write-Output ('cargo ' + ($arguments -join ' '))
& cargo @arguments
exit $LASTEXITCODE
