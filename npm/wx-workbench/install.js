'use strict';

if (process.platform !== 'win32' || process.arch !== 'x64') {
  console.error('wx-workbench supports Windows x64 only');
  process.exit(1);
}
try {
  require.resolve('@leyan2174/wx-workbench-win32-x64/bin/wx.exe');
} catch {
  console.log('wx-workbench: Windows platform package not installed');
}
