'use strict';

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');

const repo = path.resolve(__dirname, '../../..');
const asset = 'wx-workbench-windows-x86_64.exe';
const windows = process.platform === 'win32';
const read = (file) => fs.readFileSync(path.join(repo, file), 'utf8');

test('Cargo publishes only the formal wx binary', () => {
  const binaries = [...read('Cargo.toml').matchAll(/\[\[bin\]\]\s+name\s*=\s*"([^"]+)"/g)].map((match) => match[1]);
  assert.deepEqual(binaries, ['wx']);
  assert.equal(fs.existsSync(path.join(repo, 'src/toolbox_main.rs')), false);
});

test('release packaging is manual, gated, and keeps license notices', () => {
  const release = read('.github/workflows/release.yml');
  assert.doesNotMatch(release, /wx-toolbox|wx-windows-x86_64/);
  assert.doesNotMatch(release, /push:\s*\r?\n\s+tags:/);
  assert.match(release, /workflow_dispatch:/);
  assert.match(release, /publish_github_release:/);
  assert.match(release, /publish_npm:/);
  assert.match(release, /contents: read/);
  assert.match(release, /publish-github-release:[\s\S]*inputs\.publish_github_release == true[\s\S]*environment: github-release/);
  assert.match(release, /publish-npm:[\s\S]*inputs\.publish_npm == true[\s\S]*environment: npm-publish/);
  assert.equal((release.match(/startsWith\(github\.ref, 'refs\/tags\/v'\)/g) || []).length, 2);
  assert.match(release, /cargo build[^\r\n]*--bin wx --target x86_64-pc-windows-msvc/);
  assert.ok(release.includes(`Copy-Item ${asset} npm/platforms/win32-x64/bin/wx.exe`));
  assert.match(release, /Copy-Item LICENSE npm\/wx-workbench\/LICENSE/);
  assert.match(release, /Copy-Item THIRD_PARTY_NOTICES\.md npm\/platforms\/win32-x64\/THIRD_PARTY_NOTICES\.md/);
  assert.match(release, /working-directory: npm\/wx-workbench/);
});

test('both npm packages declare license and third-party notice files', () => {
  for (const file of ['npm/wx-workbench/package.json', 'npm/platforms/win32-x64/package.json']) {
    const manifest = JSON.parse(read(file));
    assert.ok(manifest.files.includes('LICENSE'), `${file} must include LICENSE`);
    assert.ok(manifest.files.includes('THIRD_PARTY_NOTICES.md'), `${file} must include THIRD_PARTY_NOTICES.md`);
  }
});

const psQuote = (value) => `'${value.replace(/'/g, "''")}'`;

function selectInstallation(assets) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'wx-release-contract-'));
  try {
    const oldDirectory = path.join(root, 'wx-cli');
    fs.mkdirSync(oldDirectory);
    const oldBinary = path.join(oldDirectory, 'wx.exe');
    fs.writeFileSync(oldBinary, 'synthetic old installation');
    const installer = read('install.ps1');
    const marker = installer.indexOf('Write-Host "版本: $Tag"');
    assert.ok(marker > 0, 'installer selection must end before the download/install stages');
    const selection = installer.slice(0, marker);
    assert.doesNotMatch(selection, /SetEnvironmentVariable|Move-Item|Remove-Item|New-Item/);
    const release = JSON.stringify({ tag_name: 'v0.3.0', assets: assets.map((name) => ({ name })) });
    const script = `
$ErrorActionPreference = 'Stop'
function Invoke-RestMethod { param($Uri) ${psQuote(release)} | ConvertFrom-Json }
function Invoke-WebRequest { throw 'network download is forbidden in this test' }
${selection}
[pscustomobject]@{ InstallDir = $InstallDir; Asset = $Asset; Repo = $Repo } | ConvertTo-Json -Compress
`;
    const powershell = path.join(process.env.SystemRoot, 'System32/WindowsPowerShell/v1.0/powershell.exe');
    const result = spawnSync(powershell, ['-NoLogo', '-NoProfile', '-NonInteractive', '-EncodedCommand', Buffer.from(script, 'utf16le').toString('base64')], {
      env: { ...process.env, LOCALAPPDATA: root }, encoding: 'utf8', windowsHide: true,
      timeout: 15000, maxBuffer: 256 * 1024,
    });
    assert.ifError(result.error);
    assert.equal(fs.readFileSync(oldBinary, 'utf8'), 'synthetic old installation');
    assert.equal(fs.existsSync(path.join(root, 'wx-workbench')), false);
    return { result, expectedDirectory: path.join(root, 'wx-workbench') };
  } finally {
    // Only remove the unique synthetic fixture created by this test.
    fs.rmSync(root, { recursive: true, force: true });
  }
}

test('installer selects the new asset and directory even when legacy ones exist', { skip: !windows }, () => {
  const { result, expectedDirectory } = selectInstallation(['wx-windows-x86_64.exe', asset]);
  assert.equal(result.status, 0, result.stderr);
  const selected = JSON.parse(result.stdout.trim().split(/\r?\n/).pop());
  assert.equal(selected.InstallDir, expectedDirectory);
  assert.equal(selected.Asset, asset);
  assert.equal(selected.Repo, 'leyan2174/wx-workbench');
});

test('installer rejects a release containing only a legacy attachment', { skip: !windows }, () => {
  const { result } = selectInstallation(['wx-windows-x86_64.exe']);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /no supported wx-workbench Windows x64 binary/);
});
