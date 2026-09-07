// 只使用公开的合成 key，执行原始 Node 包装器生成逐字节基准。
const fs = require('fs');
const path = require('path');
const {spawnSync} = require('child_process');
const runner = path.resolve(__dirname, '../../../vendor/wechat-decrypt/sns_media_wasm/weflow_wasm_keystream.js');
const cases = [];
for (const key of ['0', '1', '-1', '18446744073709551615', '00042', ' 42 ']) {
  for (const size of [1, 7, 8, 9, 16]) cases.push({id: cases.length, key, size});
}
for (const size of [1023, 1024, 1025, 131071, 131072]) cases.push({id: cases.length, key: '1', size});
const result = spawnSync(process.execPath, [runner, '--stdio'], {
  input: cases.map(c => JSON.stringify(c)).join('\n') + '\n', encoding: 'utf8', maxBuffer: 8 * 1024 * 1024,
});
if (result.error || result.status !== 0) throw Error(result.error || result.stderr);
const responses = result.stdout.trim().split('\n').map(JSON.parse);
if (responses.length !== cases.length) throw Error('Missing vectors');
const vectors = responses.map((r, i) => {
  if (r.error || r.id !== cases[i].id) throw Error('Oracle failed');
  const bytes = Buffer.from(r.data, 'base64');
  if (bytes.length !== cases[i].size) throw Error('Wrong length');
  return {...cases[i], hex: bytes.toString('hex')};
});
fs.writeFileSync(path.join(__dirname, 'vectors.json'), JSON.stringify(vectors, null, 2) + '\n');
console.log(`Generated ${vectors.length} synthetic byte-exact Node/WASM vectors`);
