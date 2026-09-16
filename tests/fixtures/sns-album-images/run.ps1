param([string]$TargetDir = '')
$ErrorActionPreference = 'Stop'
$runner = Join-Path $PSScriptRoot '../../run-module.ps1'
& $runner -TargetDir $TargetDir -TestFilter 'application::moments::album_images::tests'
exit $LASTEXITCODE
