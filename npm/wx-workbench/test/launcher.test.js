'use strict';

const assert = require('node:assert/strict');
const { spawnSync } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const packageRoot = path.resolve(__dirname, '..');
const repoRoot = path.resolve(packageRoot, '../..');
const launcher = path.join(packageRoot, 'bin/wx.js');
const windows = process.platform === 'win32' && process.arch === 'x64';

function launch(overrides) {
  const env = { ...process.env };
  delete env.WX_WORKBENCH_BINARY;
  delete env.WX_UNSUPPORTED_BINARY;
  return spawnSync(process.execPath, [launcher, '--version'], {
    env: { ...env, ...overrides }, encoding: 'utf8', timeout: 10000, windowsHide: true,
  });
}

test('supported binary override launches the selected executable', { skip: !windows }, () => {
  const result = launch({ WX_WORKBENCH_BINARY: process.execPath, WX_UNSUPPORTED_BINARY: 'not-a-real-binary' });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim(), process.version);
});

function simulate(env, available = true, args = ['--version']) {
  const calls = [];
  const errors = [];
  const exit = new Error('synthetic process exit');
  let status;
  const requireStub = (name) => {
    assert.equal(name, 'child_process');
    return { execFileSync(binary, args) { calls.push({ binary, args: Array.from(args) }); } };
  };
  requireStub.resolve = (name) => {
    assert.equal(name, '@leyan2174/wx-workbench-win32-x64/bin/wx.exe');
    if (!available) throw new Error('synthetic missing package');
    return 'synthetic-platform/wx.exe';
  };
  try {
    vm.runInNewContext(fs.readFileSync(launcher, 'utf8'), {
      require: requireStub,
      process: { platform: 'win32', arch: 'x64', env, argv: ['node', 'wx.js', ...args],
        exit(code) { status = code; throw exit; } },
      console: { error(text) { errors.push(text); } },
    }, { timeout: 1000 });
  } catch (error) {
    if (error !== exit) throw error;
  }
  return { calls, errors, status };
}

test('unsupported environment variable cannot replace the platform package', () => {
  const result = simulate({ WX_UNSUPPORTED_BINARY: 'unsupported.exe' });
  assert.deepEqual(result.calls, [{ binary: 'synthetic-platform/wx.exe', args: ['--version'] }]);
});

test('unsupported environment variable cannot rescue a missing platform package', () => {
  const result = simulate({ WX_UNSUPPORTED_BINARY: 'unsupported.exe' }, false);
  assert.equal(result.status, 1);
  assert.deepEqual(result.calls, []);
  assert.match(result.errors.join('\n'), /binary not found/);
});

test('explicit override works independently of platform package installation', () => {
  const result = simulate({ WX_WORKBENCH_BINARY: 'selected.exe', WX_UNSUPPORTED_BINARY: 'unsupported.exe' }, false);
  assert.deepEqual(result.calls, [{ binary: 'selected.exe', args: ['--version'] }]);
});

const businessCommands = [
  'setup',
  'cleanup',
  'status',
  'progress',
  'monitor',
  'latency',
  'web',
  'gui',
  'database decrypt',
  'audio transcribe-message',
  'chats export-delta',
  'chats plan',
  'audio transcribe',
  'chats transcribe-manifest',
  'media video decode',
  'moments export-snapshot',
  'chats export',
  'emoticons export',
  'chats export-all',
  'moments export',
  'chats export-messages',
  'moments archive',
  'keys image',
  'keys database',
  'keys watch-image',
  'media image decode-cache',
  'media image decode',
  'media image decode-directory',
  'audio export',
  'audio convert',
  'chats transcribe',
];

for (const command of businessCommands) {
  test(`formal command arguments pass through unchanged: wx ${command}`, () => {
    const args = [...command.split(' '), '--help'];
    const result = simulate({}, true, args);
    assert.deepEqual(result.calls, [{ binary: 'synthetic-platform/wx.exe', args }]);
    assert.deepEqual(result.errors, []);
    assert.equal(result.status, undefined);
  });
}

test('npm distribution agrees with the Rust package and platform package', () => {
  const main = JSON.parse(fs.readFileSync(path.join(packageRoot, 'package.json'), 'utf8'));
  const platform = JSON.parse(fs.readFileSync(path.join(repoRoot, 'npm/platforms/win32-x64/package.json'), 'utf8'));
  const cargo = fs.readFileSync(path.join(repoRoot, 'Cargo.toml'), 'utf8');
  assert.equal(main.name, '@leyan2174/wx-workbench');
  assert.equal(main.optionalDependencies[platform.name], platform.version);
  assert.equal(main.version, platform.version);
  assert.match(cargo, /name = "wx-workbench"/);
  assert.ok(cargo.includes(`version = "${main.version}"`));
  assert.equal(main.bin.wx, 'bin/wx.js');
});
