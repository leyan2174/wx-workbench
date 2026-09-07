param([string]$Dependencies = 'target/debug/deps')
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path "$PSScriptRoot/../../..").Path
Push-Location $root
try {
    $env:CARGO_MANIFEST_DIR = $root
    $deps = (Resolve-Path $Dependencies).Path
    $arguments = @('--edition=2021', '--test', "$PSScriptRoot/harness.rs", '-L', "dependency=$deps", '-o', "$PSScriptRoot/album-images-tests.exe")
    $windowsLib = 'C:/Users/leyan/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/windows_x86_64_msvc-0.52.6/lib'
    $arguments += @('-L', "native=$windowsLib")
    foreach ($name in @('anyhow', 'regex', 'serde_json', 'reqwest', 'tempfile', 'same_file', 'zeroize', 'windows', 'sha2', 'wasmi')) {
        $library = Get-ChildItem -LiteralPath $deps -Filter "lib$name-*.rlib" | Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if (!$library) { throw "Missing built dependency: $name" }
        $arguments += @('--extern', "$name=$($library.FullName)")
    }
    Write-Output ('rustc ' + (($arguments | ForEach-Object { '"' + $_ + '"' }) -join ' '))
    & rustc @arguments 2>&1 | Tee-Object "$PSScriptRoot/compile.log"
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    Write-Output "& '$PSScriptRoot/album-images-tests.exe' album_images::tests --test-threads=1"
    & "$PSScriptRoot/album-images-tests.exe" album_images::tests --test-threads=1 2>&1 | Tee-Object "$PSScriptRoot/tests.log"
    exit $LASTEXITCODE
} finally { Pop-Location }
