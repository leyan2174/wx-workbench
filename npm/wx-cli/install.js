'use strict';

if (process.platform !== 'win32' || process.arch !== 'x64') {
  console.error('wx-cli supports Windows x64 only');
  process.exit(1);
}
try {
  require.resolve('@jackwener/wx-cli-win32-x64/bin/wx.exe');
} catch {
  console.log('wx-cli: Windows platform package not installed');
}
