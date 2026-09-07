const fs = require('fs');
const path = require('path');
const root = path.resolve(__dirname, '../../..');
const moduleBytes = fs.readFileSync(path.join(root, 'vendor/wechat-decrypt/sns_media_wasm/wasm_video_decode.wasm'));
const moduleWasm = new WebAssembly.Module(moduleBytes);
let instance;
const text = p => {
  const bytes = new Uint8Array(instance.exports.memory.buffer);
  let end = p;
  while (bytes[end]) end++;
  return Buffer.from(bytes.subarray(p, end)).toString();
};
const imports = {env: {}, wasi_snapshot_preview1: {}};
for (const imp of WebAssembly.Module.imports(moduleWasm)) {
  imports[imp.module][imp.name] = (...args) => {
    if (imp.name.startsWith('_embind_register')) {
      let name = '';
      if (imp.name === '_embind_register_class') name = text(args[10]);
      if (imp.name === '_embind_register_class_function') name = text(args[1]);
      if (imp.name === '_embind_register_std_string') name = text(args[1]);
      console.log(imp.name, name, JSON.stringify(args));
      return;
    }
    console.log('HOST', imp.name, JSON.stringify(args));
    if (imp.name === 'environ_sizes_get') {
      const view = new DataView(instance.exports.memory.buffer);
      view.setUint32(args[0], 0, true);
      view.setUint32(args[1], 0, true);
      return 0;
    }
    if (imp.name === 'environ_get') return 0;
    if (imp.name === '__cxa_atexit') return 0;
    if (imp.name === '_embind_finalize_value_object') return;
    throw Error('Unexpected import');
  };
}
instance = new WebAssembly.Instance(moduleWasm, imports);
console.log('memory', instance.exports.memory.buffer.byteLength);
instance.exports.__wasm_call_ctors();
