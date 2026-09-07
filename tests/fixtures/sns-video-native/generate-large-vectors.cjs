// 使用原始 Node 包装器生成合成边界 oracle；仅保存完整输出的 SHA-256，避免提交大块密钥流。
const fs = require('fs');
const path = require('path');
const crypto = require('crypto');
const { spawnSync } = require('child_process');
const runner = path.resolve(__dirname, '../../../vendor/wechat-decrypt/sns_media_wasm/weflow_wasm_keystream.js');
const sizes = [131071, 131072, 131073, 26214399, 26214400, 26214401];
const vectors = [];
for (const size of sizes) {
  const result = spawnSync(process.execPath, [runner, '42', String(size)], {
    encoding: 'utf8', maxBuffer: 40 * 1024 * 1024, timeout: 30000, windowsHide: true,
  });
  if (result.error || result.status !== 0) throw new Error('Synthetic Node oracle failed');
  const bytes = Buffer.from(result.stdout, 'base64');
  if (bytes.length !== size) throw new Error('Unexpected oracle length');
  const vector = {key: '42', size, sha256: crypto.createHash('sha256').update(bytes).digest('hex')};
  vectors.push(vector);
  console.log(JSON.stringify(vector));
}
fs.writeFileSync(path.join(__dirname, 'large-vectors.json'), JSON.stringify(vectors, null, 2) + '\n');
