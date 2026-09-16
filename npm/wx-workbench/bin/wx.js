#!/usr/bin/env node
'use strict';

const { execFileSync } = require('child_process');
const PLATFORM_PACKAGE = '@leyan2174/wx-workbench-win32-x64';

const platformKey = `${process.platform}-${process.arch}`;
if (platformKey !== 'win32-x64') {
  console.error('wx-workbench supports Windows x64 only');
  process.exit(1);
}

function getBinaryPath() {
  if (process.env.WX_WORKBENCH_BINARY) {
    return process.env.WX_WORKBENCH_BINARY;
  }

  try {
    return require.resolve(`${PLATFORM_PACKAGE}/bin/wx.exe`);
  } catch {
    console.error(`wx-workbench: binary not found for ${platformKey}`);
    console.error('Try: npm install -g @leyan2174/wx-workbench');
    process.exit(1);
  }
}

try {
  execFileSync(getBinaryPath(), process.argv.slice(2), {
    stdio: 'inherit',
    env: { ...process.env },
  });
} catch (e) {
  if (e && e.status != null) process.exit(e.status);
  throw e;
}
