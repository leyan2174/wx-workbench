param([string]$TargetDir = 'C:/CodexLocal/build/mcp-image', [switch]$Check, [string]$TestFilter = 'daemon::query::mcp_image::tests')
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '../../..')).Path
$stage = Join-Path 'C:/CodexLocal/build' ('mcp-image-source-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
Write-Output "临时源码副本: $stage"
Copy-Item -LiteralPath (Join-Path $root 'src') -Destination $stage -Recurse
foreach ($name in @('Cargo.toml','Cargo.lock','build.rs')) {
    Copy-Item -LiteralPath (Join-Path $root $name) -Destination $stage
}
$testsRoot = Join-Path $root 'tests'
Get-ChildItem -LiteralPath $testsRoot -File -Recurse | Where-Object {
    $_.Extension -in @('.rs','.json','.silk','.db','.enc','.wasm')
} | ForEach-Object {
    $relative = [IO.Path]::GetRelativePath($root,$_.FullName)
    $target = Join-Path $stage $relative
    New-Item -ItemType Directory -Path (Split-Path $target -Parent) -Force | Out-Null
    Copy-Item -LiteralPath $_.FullName -Destination $target
}
$wasm = 'vendor/wechat-decrypt/sns_media_wasm/wasm_video_decode.wasm'
New-Item -ItemType Directory -Path (Split-Path (Join-Path $stage $wasm) -Parent) -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $root $wasm) -Destination (Join-Path $stage $wasm)
# 只验证 main 的真实注册，不在副本中补写模块或接口。
foreach ($entry in @(@('src/attachment/mod.rs','native_image'),@('src/attachment/mod.rs','local_files'),@('src/daemon/query.rs','mcp_image'))) {
    $path = Join-Path $stage $entry[0]
    $text = [IO.File]::ReadAllText($path)
    if ($text -notmatch ('(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+' + $entry[1] + '\s*;')) {
        throw "生产模块尚未注册: $($entry[1])"
    }
}
$cache = Join-Path $stage 'src/daemon/cache.rs'
if ([IO.File]::ReadAllText($cache) -notmatch 'fn raw_db_keys\s*\(') {
    throw '生产 DbCache 缺少 raw_db_keys；停止测试，不注入替代接口。'
}
$sourceHash = (Get-FileHash -LiteralPath (Join-Path $root 'src/daemon/cache.rs') -Algorithm SHA256).Hash
$copyHash = (Get-FileHash -LiteralPath $cache -Algorithm SHA256).Hash
if ($sourceHash -ne $copyHash) { throw 'cache.rs 在复制期间变化，请重新运行。' }
Write-Output "使用真实 raw_db_keys，cache.rs 原样复制，SHA256: $copyHash"
$env:LIBCLANG_PATH = 'C:/CodexLocal/build-tools/libclang/clang/native'
$manifest = Join-Path $stage 'Cargo.toml'
if ($Check) {
    Write-Output "cargo check --offline --manifest-path $manifest --target x86_64-pc-windows-msvc --target-dir $TargetDir --bin wx"
    & cargo check --offline --manifest-path $manifest --target x86_64-pc-windows-msvc --target-dir $TargetDir --bin wx 2>&1 | Tee-Object (Join-Path $PSScriptRoot 'host-wrapper-check.log')
} else {
    Write-Output "cargo test --offline --manifest-path $manifest --target x86_64-pc-windows-msvc --target-dir $TargetDir --bin wx $TestFilter -- --nocapture"
    & cargo test --offline --manifest-path $manifest --target x86_64-pc-windows-msvc --target-dir $TargetDir --bin wx $TestFilter -- --nocapture 2>&1 | Tee-Object (Join-Path $PSScriptRoot 'host-wrapper-tests.log')
}
$code = $LASTEXITCODE
Write-Output "执行退出码: $code"
exit $code
