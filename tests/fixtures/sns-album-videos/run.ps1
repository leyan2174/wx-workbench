param([string]$TargetDir = '')
$ErrorActionPreference = 'Stop'
$runner = Join-Path $PSScriptRoot '../../run-module.ps1'
& $runner -TargetDir $TargetDir -TestFilter 'toolkit::sns::album_videos::tests'
exit $LASTEXITCODE
