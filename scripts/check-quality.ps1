#requires -Version 7.0
[CmdletBinding()]
param(
    [ValidateSet('Check', 'Test', 'Fixtures', 'All')]
    [string]$Stage = 'Check',
    [switch]$Offline
)

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$repoRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$targetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $repoRoot 'target' }
if (-not [IO.Path]::IsPathRooted($targetRoot)) { $targetRoot = Join-Path $repoRoot $targetRoot }
$targetRoot = [IO.Path]::GetFullPath($targetRoot)
$ownerFile = Join-Path $targetRoot '.checkout-owner'
# Keep this ownership policy aligned with check-fixtures.ps1.
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
    } finally { $ownerStream.Dispose() }
}

$round = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ') + '-' + [Guid]::NewGuid().ToString('N')
$logRoot = Join-Path $targetRoot "quality-checks/$round"
New-Item -ItemType Directory -Force -Path $logRoot | Out-Null
$results = [Collections.Generic.List[object]]::new()
$started = [DateTime]::UtcNow.ToString('o')
$failed = $false
$previousTarget = $env:CARGO_TARGET_DIR
$lock = $null
$before = $null
$after = $null

function Format-Command([string]$Executable, [string[]]$Arguments) {
    '& ' + ((@($Executable) + $Arguments | ForEach-Object { "'" + $_.Replace("'", "''") + "'" }) -join ' ')
}

function Write-Bounded([string]$Message) {
    $utf8 = [Text.Encoding]::UTF8
    if ($utf8.GetByteCount($Message) -gt 4000) {
        $Message = $utf8.GetString($utf8.GetBytes($Message), 0, 3900) + "`n[summary truncated; see full logs]"
    }
    Write-Host $Message
}

function Invoke-Logged([string]$Name, [string]$Executable, [string[]]$Arguments) {
    $log = Join-Path $logRoot "$Name.log"
    $command = Format-Command $Executable $Arguments
    $begin = [DateTime]::UtcNow.ToString('o')
    Write-Host $command
    $code = 1
    try {
        $global:LASTEXITCODE = 0
        & $Executable @Arguments *> $log
        $code = $LASTEXITCODE
    } catch {
        $_ | Out-String | Add-Content -LiteralPath $log -Encoding utf8
    }
    $end = [DateTime]::UtcNow.ToString('o')
    $results.Add([pscustomobject]@{
        step = $Name; command = $command; started_utc = $begin; ended_utc = $end
        exit_code = $code; result = $(if ($code -eq 0) { 'passed' } else { 'failed' }); log = $log
    })
    # Read only a bounded byte tail, even when a tool emits a very long line.
    $stream = [IO.File]::OpenRead($log)
    try {
        $count = [int][Math]::Min(1800, $stream.Length)
        [void]$stream.Seek(-$count, [IO.SeekOrigin]::End)
        $buffer = [byte[]]::new($count)
        $read = $stream.Read($buffer, 0, $count)
        $tail = [Text.Encoding]::UTF8.GetString($buffer, 0, $read)
    } finally { $stream.Dispose() }
    Write-Bounded "$Name : exit $code; started $begin; ended $end`nFull log: $log`nOutput tail (possibly truncated):`n$tail"
    if ($code -ne 0) { $script:failed = $true }
}

function Get-SourceSnapshot([string]$Name) {
    Invoke-Logged "$Name-diff" 'git' @('diff', '--no-ext-diff', '--binary', 'HEAD', '--', '.')
    $diffOK = $results[$results.Count - 1].exit_code -eq 0
    Invoke-Logged "$Name-status" 'git' @('status', '--porcelain=v1', '--untracked-files=all')
    $statusOK = $results[$results.Count - 1].exit_code -eq 0
    Invoke-Logged "$Name-untracked" 'git' @('ls-files', '--others', '--exclude-standard', '-z', '--', '*.rs')
    $untrackedOK = $results[$results.Count - 1].exit_code -eq 0
    $untrackedLog = Join-Path $logRoot "$Name-untracked.log"
    $hashLog = Join-Path $logRoot "$Name-untracked-hashes.json"
    $hashes = @(foreach ($path in ([IO.File]::ReadAllText($untrackedLog) -split "`0")) {
        if (-not [string]::IsNullOrWhiteSpace($path)) {
            [pscustomobject]@{
                path = $path
                sha256 = (Get-FileHash -LiteralPath (Join-Path $repoRoot $path) -Algorithm SHA256).Hash
            }
        }
    })
    ConvertTo-Json -InputObject $hashes | Set-Content -LiteralPath $hashLog -Encoding utf8
    return [pscustomobject]@{
        valid = $diffOK -and $statusOK -and $untrackedOK
        diff_sha256 = (Get-FileHash -LiteralPath (Join-Path $logRoot "$Name-diff.log") -Algorithm SHA256).Hash
        status_sha256 = (Get-FileHash -LiteralPath (Join-Path $logRoot "$Name-status.log") -Algorithm SHA256).Hash
        untracked_rs_sha256 = (Get-FileHash -LiteralPath $hashLog -Algorithm SHA256).Hash
    }
}

Push-Location $repoRoot
try {
    $env:CARGO_TARGET_DIR = $targetRoot
    # Existing fixture logs have fixed names. Serialize this entry point until archived.
    $lock = [IO.File]::Open((Join-Path $targetRoot '.quality-check.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    $before = Get-SourceSnapshot 'before'
    $common = @('--locked', '--target', 'x86_64-pc-windows-msvc', '--all-targets')
    if ($Offline) { $common += '--offline' }
    if ($Stage -eq 'All') { Invoke-Logged 'fmt' 'cargo' @('fmt', '--all', '--', '--check') }
    if ($Stage -in @('Check', 'All')) {
        Invoke-Logged 'check' 'cargo' (@('check') + $common)
        Invoke-Logged 'clippy' 'cargo' (@('clippy') + $common + @('--', '-D', 'warnings'))
    }
    if ($Stage -in @('Test', 'All')) { Invoke-Logged 'test' 'cargo' (@('test') + $common + @('--no-fail-fast', '--', '--test-threads=1')) }
    if ($Stage -in @('Fixtures', 'All')) {
        $shell = (Get-Process -Id $PID).Path
        $fixtureArgs = @('-NoProfile', '-NonInteractive', '-File', (Join-Path $PSScriptRoot 'check-fixtures.ps1'))
        if ($Offline) { $fixtureArgs += '-Offline' }
        Invoke-Logged 'fixtures' $shell $fixtureArgs
        $fixtureLogs = Join-Path $targetRoot 'quality-fixtures'
        if (Test-Path -LiteralPath $fixtureLogs) {
            Copy-Item -LiteralPath $fixtureLogs -Destination (Join-Path $logRoot 'fixture-logs') -Recurse
        }
    }
    $after = Get-SourceSnapshot 'after'
} catch {
    $failed = $true
    $_ | Out-String | Set-Content -LiteralPath (Join-Path $logRoot 'error.log') -Encoding utf8
    Write-Bounded "Quality runner failed; full error: $(Join-Path $logRoot 'error.log')"
} finally {
    $state = 'unknown'
    if ($before -and $after -and $before.valid -and $after.valid) {
        $state = if ($before.diff_sha256 -ne $after.diff_sha256 -or $before.status_sha256 -ne $after.status_sha256 -or $before.untracked_rs_sha256 -ne $after.untracked_rs_sha256) { 'changed-intermediate-state' } else { 'no-change-observed' }
    }
    $summary = [pscustomobject]@{
        stage = $Stage; offline = [bool]$Offline; checkout = $repoRoot; target = $targetRoot
        started_utc = $started; ended_utc = [DateTime]::UtcNow.ToString('o')
        exit_code = $(if ($failed) { 1 } else { 0 }); source_state = $state
        acceptance = 'Observational checks only, not stable acceptance: snapshots cannot detect reverted edits, ignored file changes, or content-only changes to untracked non-Rust files.'
        before = $before; after = $after; steps = @($results.ToArray())
    }
    try {
        $summary | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $logRoot 'summary.json') -Encoding utf8
        Write-Bounded "Stage $Stage : exit $($summary.exit_code); source state: $state`n$($summary.acceptance)`nFull logs and summary: $logRoot"
    } finally {
        if ($lock) { $lock.Dispose() }
        $env:CARGO_TARGET_DIR = $previousTarget
        Pop-Location
    }
}
if ($failed) { exit 1 }
exit 0
