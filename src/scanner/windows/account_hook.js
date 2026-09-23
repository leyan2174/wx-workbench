// Adapted from LOGO127/wechat-ai-memory; upstream license not independently verified.
// Copyright (c) 2026 WeChat AI Memory contributors. See THIRD_PARTY_NOTICES.md.
'use strict';
let installed = false;
function report(result, data) {
    send({type: 'wx-account', id: 0, result, returns: null}, data);
}
function install(module) {
    if (installed) return;
    installed = true;
    try {
        const constants = Memory.scanSync(module.base, module.size, '22 ae 28 d7 98 2f 8a 42');
        if (!constants.length) throw new Error('constants');
        const table = constants[0].address;
        let reference = null;
        for (const opcode of ['48 8d', '4c 8d']) {
            for (const modrm of ['05', '0d', '15', '1d', '25', '2d', '35', '3d']) {
                if (reference !== null) break;
                for (const result of Memory.scanSync(module.base, module.size, opcode + ' ' + modrm)) {
                    if (result.address.add(7).add(result.address.add(3).readS32()).equals(table)) {
                        reference = result.address; break;
                    }
                }
            }
        }
        if (reference === null) throw new Error('reference');
        let entry = null;
        for (let offset = 0; offset < 8192; offset++) {
            const address = reference.sub(offset);
            if (address.compare(module.base) <= 0) break;
            try { if (address.sub(1).readU8() === 0xcc) { entry = address; break; } } catch (_) {}
        }
        if (entry === null) throw new Error('entry');
        const seen = new Set();
        Interceptor.attach(entry, {
            onEnter() {
                try {
                    const block = new Uint8Array(this.context.rdx.readByteArray(128));
                    for (let i = 32; i < 128; i++) if (block[i] !== 0x36) return;
                    const key = new Uint8Array(32);
                    for (let i = 0; i < 32; i++) key[i] = block[i] ^ 0x36;
                    const identity = Array.from(key).join(',');
                    if (seen.has(identity)) { key.fill(0); return; }
                    if (seen.size >= 128) { key.fill(0); return; }
                    seen.add(identity);
                    report('candidate', key.buffer);
                    key.fill(0);
                } catch (_) {}
            }
        });
        report('ready');
    } catch (_) { report('error'); }
}
const loaded = Process.findModuleByName('Weixin.dll');
if (loaded) install(loaded);
else Process.attachModuleObserver({ onAdded(module) {
    if (module.name.toLowerCase() === 'weixin.dll') install(module);
}});
