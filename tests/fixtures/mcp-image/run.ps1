param([string]$TargetDir = '', [switch]$Check, [string]$TestFilter = 'daemon::query::mcp_image::tests')
$ErrorActionPreference = 'Stop'
$runner = Join-Path $PSScriptRoot '../../run-module.ps1'
& $runner -TargetDir $TargetDir -TestFilter $TestFilter -Check:$Check
exit $LASTEXITCODE
