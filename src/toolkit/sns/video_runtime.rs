//! 离线承载供应商 WxIsaac64 WASM；不提供网络、账户或文件系统宿主能力。
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};
use wasmi::{
    Caller, Config, Engine, ExternType, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, Val,
};
use zeroize::Zeroize;

pub const VIDEO_PREFIX_BYTES: usize = 128 * 1024;
pub const MAX_KEYSTREAM_BYTES: usize = 25 * 1024 * 1024;
const BASE_FUEL: u64 = 100_000_000;
const MAX_FUEL: u64 = 300_000_000;
const WASM_MAX_BYTES: usize = 4 * 1024 * 1024;
const KEY_MAX_BYTES: usize = 1024;
const WASM_SHA256: [u8; 32] = [
    0xdc, 0xa7, 0x96, 0xba, 0xce, 0xc3, 0x7d, 0x85, 0x22, 0xc7, 0x98, 0x3b, 0x39, 0x45, 0xe5, 0xd5,
    0x79, 0xbd, 0x74, 0x16, 0x4e, 0x3b, 0x21, 0xf0, 0xeb, 0xc7, 0x73, 0xbe, 0x6d, 0xfc, 0x8b, 0x6e,
];

#[derive(Clone, Copy, Debug)]
pub struct RuntimeLimits {
    /// 每次调用的硬上限；实际预算还按请求长度收紧，绝不超过 MAX_FUEL。
    pub fuel: u64,
    pub memory_bytes: usize,
    pub input_bytes: usize,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            fuel: MAX_FUEL,
            memory_bytes: 64 * 1024 * 1024,
            input_bytes: 256 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoRuntimeError {
    InvalidInput,
    AssetRead,
    UnsupportedAsset,
    Runtime,
    InvalidMp4,
}

impl std::fmt::Display for VideoRuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "SNS video input exceeds limits or is empty",
            Self::AssetRead => "SNS video WASM asset could not be read",
            Self::UnsupportedAsset => "SNS video WASM asset does not match the supported ABI",
            Self::Runtime => "SNS video WASM failed or exhausted runtime limits",
            Self::InvalidMp4 => "SNS video decoded header is not MP4",
        })
    }
}
impl std::error::Error for VideoRuntimeError {}
type Result<T> = std::result::Result<T, VideoRuntimeError>;

/// 每次调用创建隔离 Store，密钥不进入 Debug、日志或持久缓存。
pub struct VideoRuntime {
    engine: Engine,
    module: Module,
    limits: RuntimeLimits,
}

#[derive(Default)]
struct Binding {
    class: i32,
    constructor: Option<(i32, i32)>,
    generate: Option<(i32, i32)>,
    destructor: Option<i32>,
}
struct Host {
    limits: StoreLimits,
    binding: Binding,
    captured: Option<Vec<u8>>,
    expected: usize,
}

fn failure(_: impl std::fmt::Display) -> VideoRuntimeError {
    VideoRuntimeError::Runtime
}
fn host_error() -> wasmi::Error {
    wasmi::Error::new("unsupported or invalid SNS WASM host call")
}
fn memory(c: &Caller<'_, Host>) -> std::result::Result<Memory, wasmi::Error> {
    c.get_export("memory")
        .and_then(|e| e.into_memory())
        .ok_or_else(host_error)
}
fn word(c: &Caller<'_, Host>, p: usize) -> std::result::Result<i32, wasmi::Error> {
    let mut b = [0; 4];
    memory(c)?.read(c, p, &mut b).map_err(|_| host_error())?;
    Ok(i32::from_le_bytes(b))
}
fn named(c: &Caller<'_, Host>, p: i32, name: &[u8]) -> bool {
    let Ok(mem) = memory(c) else {
        return false;
    };
    let p = p as u32 as usize;
    mem.data(c).get(p..p.saturating_add(name.len())) == Some(name)
}

// 回调长度必须等于本次对齐后的申请；在分配宿主缓冲区之前核验 guest 地址。
fn capture_range(
    start: usize,
    size: usize,
    expected: usize,
    memory_bytes: usize,
) -> std::result::Result<std::ops::Range<usize>, wasmi::Error> {
    if size == 0 || size != expected || size > MAX_KEYSTREAM_BYTES || !size.is_multiple_of(8) {
        return Err(host_error());
    }
    let end = start.checked_add(size).ok_or_else(host_error)?;
    if end > memory_bytes {
        return Err(host_error());
    }
    Ok(start..end)
}

fn keystream_fuel(aligned: usize, limit: u64) -> u64 {
    // 已审计 WASM 的 25 MiB 合成请求消耗约 227M；视频仍只分配原来的 100M。
    let extra = aligned
        .min(MAX_KEYSTREAM_BYTES)
        .saturating_sub(VIDEO_PREFIX_BYTES) as u64;
    (BASE_FUEL + extra * 8).min(MAX_FUEL).min(limit)
}

fn host_call(
    mut c: Caller<'_, Host>,
    name: &str,
    args: &[Val],
    results: &mut [Val],
) -> std::result::Result<(), wasmi::Error> {
    let a = |i: usize| args.get(i).and_then(Val::i32).ok_or_else(host_error);
    match name {
        "_embind_register_class" => {
            if named(&c, a(10)?, b"WxIsaac64\0") {
                c.data_mut().binding.class = a(0)?;
                c.data_mut().binding.destructor = Some(a(12)?);
            }
        }
        "_embind_register_class_constructor" => {
            if a(0)? == c.data().binding.class && a(1)? == 2 {
                c.data_mut().binding.constructor = Some((a(4)?, a(5)?));
            }
        }
        "_embind_register_class_function" => {
            if a(0)? == c.data().binding.class && a(2)? == 3 && named(&c, a(1)?, b"generate\0") {
                c.data_mut().binding.generate = Some((a(5)?, a(6)?));
            }
        }
        n if n.starts_with("_embind_register_") || n == "_embind_finalize_value_object" => {}
        "__cxa_atexit" | "environ_get" => {
            results[0] = Val::I32(0);
        }
        "environ_sizes_get" => {
            let mem = memory(&c)?;
            mem.write(&mut c, a(0)? as u32 as usize, &[0; 4])
                .map_err(|_| host_error())?;
            mem.write(&mut c, a(1)? as u32 as usize, &[0; 4])
                .map_err(|_| host_error())?;
            results[0] = Val::I32(0);
        }
        "emscripten_asm_const_int" => {
            // 仅允许 JS ASM_CONSTS 中 wasm_isaac_generate 的 ii 回调。
            if a(0)? != 434460 || !named(&c, a(1)?, b"ii\0") || c.data().captured.is_some() {
                return Err(host_error());
            }
            let p = a(2)? as u32 as usize;
            let start = word(&c, p)? as u32 as usize;
            let n = word(&c, p.checked_add(4).ok_or_else(host_error)?)? as u32 as usize;
            let mem = memory(&c)?;
            let range = capture_range(start, n, c.data().expected, mem.data(&c).len())?;
            let bytes = mem.data(&c)[range].to_vec();
            c.data_mut().captured = Some(bytes);
            results[0] = Val::I32(0);
        }
        // 超过预分配堆时拒绝扩容；绝不回退到宿主文件、时间、环境或 JS。
        "emscripten_resize_heap" => {
            results[0] = Val::I32(0);
        }
        _ => return Err(host_error()),
    }
    Ok(())
}

impl VideoRuntime {
    /// 只读取显式指定的 WASM 文件，并在编译前校验大小及已审计 ABI 哈希。
    pub fn new(wasm_path: &Path, limits: RuntimeLimits) -> Result<Self> {
        let mut bytes = Vec::new();
        File::open(wasm_path)
            .map_err(|_| VideoRuntimeError::AssetRead)?
            .take((WASM_MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| VideoRuntimeError::AssetRead)?;
        Self::from_bytes(&bytes, limits)
    }

    /// 内嵌同一已审计模块；安装后不依赖 Node 或供应商源码目录。
    pub fn bundled(limits: RuntimeLimits) -> Result<Self> {
        Self::from_bytes(
            include_bytes!("../../../vendor/wechat-decrypt/sns_media_wasm/wasm_video_decode.wasm"),
            limits,
        )
    }

    fn from_bytes(bytes: &[u8], limits: RuntimeLimits) -> Result<Self> {
        if bytes.len() > WASM_MAX_BYTES || Sha256::digest(bytes)[..] != WASM_SHA256 {
            return Err(VideoRuntimeError::UnsupportedAsset);
        }
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = Module::new(&engine, bytes).map_err(failure)?;
        Ok(Self {
            engine,
            module,
            limits,
        })
    }

    pub fn keystream(&self, key: &str, size: usize) -> Result<Vec<u8>> {
        if key.len() > KEY_MAX_BYTES {
            return Err(VideoRuntimeError::InvalidInput);
        }
        // JS String.trim 额外包含 BOM，但不包含 Rust trim 接受的 NEL。
        let key =
            key.trim_matches(|c: char| c == '\u{feff}' || (c.is_whitespace() && c != '\u{85}'));
        if key.is_empty()
            || size == 0
            || size > MAX_KEYSTREAM_BYTES
            || size > self.limits.input_bytes
        {
            return Err(VideoRuntimeError::InvalidInput);
        }
        let aligned = size.checked_add(7).ok_or(VideoRuntimeError::InvalidInput)? & !7;
        if aligned > MAX_KEYSTREAM_BYTES {
            return Err(VideoRuntimeError::InvalidInput);
        }
        let host = Host {
            limits: StoreLimitsBuilder::new()
                .memory_size(self.limits.memory_bytes)
                .table_elements(4096)
                .instances(1)
                .memories(1)
                .tables(1)
                .build(),
            binding: Binding::default(),
            captured: None,
            expected: aligned,
        };
        let mut store = Store::new(&self.engine, host);
        store.limiter(|h| &mut h.limits);
        let fuel = keystream_fuel(aligned, self.limits.fuel);
        store.set_fuel(fuel).map_err(failure)?;
        let mut linker = Linker::new(&self.engine);
        for import in self.module.imports() {
            let ExternType::Func(ty) = import.ty() else {
                return Err(VideoRuntimeError::UnsupportedAsset);
            };
            let name = import.name().to_owned();
            linker
                .func_new(
                    import.module(),
                    import.name(),
                    ty.clone(),
                    move |c, a, r| host_call(c, &name, a, r),
                )
                .map_err(failure)?;
        }
        let instance = linker
            .instantiate(&mut store, &self.module)
            .map_err(failure)?
            .start(&mut store)
            .map_err(failure)?;
        let mem = instance
            .get_memory(&store, "memory")
            .ok_or(VideoRuntimeError::Runtime)?;
        let outcome = (|| {
            instance
                .get_typed_func::<(), ()>(&store, "__wasm_call_ctors")
                .map_err(failure)?
                .call(&mut store, ())
                .map_err(failure)?;
            let (ctor, raw) = store
                .data()
                .binding
                .constructor
                .ok_or(VideoRuntimeError::UnsupportedAsset)?;
            let (generate, context) = store
                .data()
                .binding
                .generate
                .ok_or(VideoRuntimeError::UnsupportedAsset)?;
            let destructor = store
                .data()
                .binding
                .destructor
                .ok_or(VideoRuntimeError::UnsupportedAsset)?;
            let ptr = instance
                .get_typed_func::<i32, i32>(&store, "malloc")
                .map_err(failure)?
                .call(&mut store, (key.len() + 5) as i32)
                .map_err(failure)?;
            if ptr <= 0 {
                return Err(VideoRuntimeError::Runtime);
            }
            let p = ptr as usize;
            mem.write(&mut store, p, &(key.len() as u32).to_le_bytes())
                .map_err(failure)?;
            mem.write(&mut store, p + 4, key.as_bytes())
                .map_err(failure)?;
            mem.write(&mut store, p + 4 + key.len(), &[0])
                .map_err(failure)?;
            let object = instance
                .get_typed_func::<(i32, i32, i32), i32>(&store, "dynCall_iii")
                .map_err(failure)?
                .call(&mut store, (ctor, raw, ptr))
                .map_err(failure)?;
            mem.data_mut(&mut store)[p..p + key.len() + 5].zeroize();
            instance
                .get_typed_func::<i32, ()>(&store, "free")
                .map_err(failure)?
                .call(&mut store, ptr)
                .map_err(failure)?;
            instance
                .get_typed_func::<(i32, i32, i32, i32), ()>(&store, "dynCall_viii")
                .map_err(failure)?
                .call(&mut store, (generate, context, object, aligned as i32))
                .map_err(failure)?;
            instance
                .get_typed_func::<(i32, i32), ()>(&store, "dynCall_vi")
                .map_err(failure)?
                .call(&mut store, (destructor, object))
                .map_err(failure)?;
            let mut out = store
                .data_mut()
                .captured
                .take()
                .ok_or(VideoRuntimeError::Runtime)?;
            out.reverse();
            out[size..].zeroize();
            out.truncate(size);
            Ok(out)
        })();
        // 正常、异常和燃料耗尽都擦除 guest 中的密钥、对象及中间结果。
        mem.data_mut(&mut store).zeroize();
        if let Some(bytes) = store.data_mut().captured.as_mut() {
            bytes.zeroize();
        }
        #[cfg(test)]
        eprintln!(
            "keystream size={size} fuel_budget={fuel} fuel_used={} success={}",
            fuel - store.get_fuel().unwrap_or(0),
            outcome.is_ok()
        );
        outcome
    }

    /// 与原视频分支一致：明文 MP4 保持原样，否则仅解码前 128 KiB 并验证 ftyp。
    pub fn decode(&self, key: &str, data: &[u8]) -> Result<Vec<u8>> {
        if data.is_empty() || data.len() > self.limits.input_bytes {
            return Err(VideoRuntimeError::InvalidInput);
        }
        let is_mp4 = |b: &[u8]| b.len() >= 12 && &b[4..8] == b"ftyp";
        if is_mp4(data) {
            return Ok(data.to_vec());
        }
        let mut stream = self.keystream(key, data.len().min(VIDEO_PREFIX_BYTES))?;
        let mut output = data.to_vec();
        for (byte, mask) in output.iter_mut().zip(&stream) {
            *byte ^= mask;
        }
        stream.zeroize();
        if !is_mp4(&output) {
            output.zeroize();
            return Err(VideoRuntimeError::InvalidMp4);
        }
        Ok(output)
    }
}

#[cfg(test)]
#[path = "video_runtime_tests.rs"]
mod tests;
