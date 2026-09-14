//! 固定账号的清理计划与显式执行；五个旧目录仅作有界元数据统计。
//!
//! 原生来源：当前 runtime 的缓存索引，或显式提供且绑定 runtime_id 的产物清单。
//! 旧产物：仅接受 cleanup adoption JSON 中逐个列出的固定分类相对文件，另需授权和账号确认。
//! 文件 ID 不是分类 ID；执行必须逐个选择计划中的完整文件 ID。所有目录、来源清单与未知文件保留。

#[path = "cleanup/handles.rs"]
mod handles;

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant},
};

use crate::{attachment::local_files::HostOutputGuard, config::Config, runtime::RuntimeContext};
use handles::{absolute, safe_component, PinnedFile, MAX_JSON_BYTES};
pub use handles::{DirectoryIdentity, Fingerprint};

const VERSION: u32 = 1;
const MAX_FILES: usize = 20_000;
const MAX_SELECTED: usize = 2048;
const MAX_INPUTS: usize = 128;
const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
const POLICY: &str = "fixed-runtime-exact-files-v2; metadata-only-fixed-root-usage; no-links; explicit-current-key-only; no-source-delete; keep-directories-and-inventories";
const MAX_USAGE_ENTRIES: usize = 20_000;
const MAX_USAGE_DEPTH: usize = 64;
const MAX_USAGE_TIME: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    RuntimeCache,
    DecryptedDatabases,
    VoiceWavCache,
    DecodedImages,
    ExportedChats,
    LegacyExports,
    RuntimeWebExports,
    KeyCache,
}

impl Category {
    pub fn id(self) -> &'static str {
        match self {
            Self::RuntimeCache => "runtime-cache",
            Self::DecryptedDatabases => "decrypted-databases",
            Self::VoiceWavCache => "voice-wav-cache",
            Self::DecodedImages => "decoded-images",
            Self::ExportedChats => "exported-chats",
            Self::LegacyExports => "legacy-exports",
            Self::RuntimeWebExports => "runtime-web-exports",
            Self::KeyCache => "key-cache",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::RuntimeCache => "原生账号数据库缓存",
            Self::DecryptedDatabases => "解密数据库",
            Self::VoiceWavCache => "语音 WAV 缓存",
            Self::DecodedImages => "图片解码缓存",
            Self::ExportedChats => "导出聊天记录",
            Self::LegacyExports => "旧格式导出",
            Self::RuntimeWebExports => "当前账号 Web 导出产物",
            Self::KeyCache => "当前账号密钥缓存（默认保护）",
        }
    }

    fn legacy(self) -> bool {
        matches!(
            self,
            Self::DecryptedDatabases
                | Self::VoiceWavCache
                | Self::DecodedImages
                | Self::ExportedChats
                | Self::LegacyExports
        )
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanInputs {
    /// 仅接受固定分类下的 _directory_export.json 或 _cleanup_inventory.json。
    #[serde(default)]
    pub native_inventories: Vec<PathBuf>,
    /// 显式旧文件接管清单，不是可递归扫描的根目录。
    pub adoption_manifest: Option<PathBuf>,
}

/// 接管仅表达用户声明的归属，不伪装成经过原生生产者验证的来源。
/// JSON 形状：{"version":1,"runtime_id":"完整账号ID","authorization":"explicit-files-only",
/// "files":[{"category":"voice-wav-cache","relative_path":"contact/voice.wav"}]}。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptionManifest {
    pub version: u32,
    pub runtime_id: String,
    pub authorization: String,
    pub files: Vec<AdoptedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptedFile {
    pub category: Category,
    pub relative_path: PathBuf,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CategoryStatus {
    pub id: Category,
    pub label: String,
    pub path: PathBuf,
    pub policy: String,
    pub inspected: bool,
    pub count: usize,
    pub bytes: u64,
    pub missing_count: usize,
    /// 仅表示尝试元数据统计；是否完整见 disk_usage.partial，不表示读取内容或可删除。
    pub unlisted_files_inspected: bool,
    pub disk_usage: Option<DiskUsage>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskUsage {
    /// 五个固定旧目录内普通文件的逻辑长度，不是分配空间或可回收空间。
    pub count: usize,
    pub bytes: u64,
    pub entries_visited: usize,
    pub skipped_count: usize,
    pub partial: bool,
    pub errors: Vec<Issue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Issue {
    pub category: Option<Category>,
    pub path: PathBuf,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub path: PathBuf,
    pub kind: String,
    pub fingerprint: Fingerprint,
    pub directories: Vec<DirectoryIdentity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedFile {
    pub id: String,
    pub category: Category,
    pub path: PathBuf,
    pub origin: String,
    pub evidence: PathBuf,
    pub fingerprint: Fingerprint,
    pub directories: Vec<DirectoryIdentity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub version: u32,
    pub mode: String,
    pub runtime_id: String,
    pub runtime_directory: PathBuf,
    pub config_path: PathBuf,
    pub policy: String,
    pub inputs: PlanInputs,
    pub key_removal_authorized: bool,
    pub categories: Vec<CategoryStatus>,
    pub evidence: Vec<Evidence>,
    pub files: Vec<PlannedFile>,
    pub count: usize,
    pub bytes: u64,
    /// 与 count/bytes（删除候选）分开；只统计五个固定旧目录，不含密钥。
    pub disk_usage: DiskUsage,
    pub errors: Vec<Issue>,
    pub plan_id: String,
}

impl Plan {
    fn seal(&self) -> Result<String> {
        let mut value = self.clone();
        value.plan_id.clear();
        // 只读空间观察不是删除依据，未知文件变化不改变已绑定文件的执行身份。
        value.disk_usage = DiskUsage::default();
        for category in &mut value.categories {
            category.disk_usage = None;
            category.unlisted_files_inspected = false;
        }
        Ok(hash(&serde_json::to_vec(&value)?))
    }
}

pub struct ExecuteOptions {
    pub select: Vec<String>,
    pub confirm_account: String,
    /// 每次执行都必须再次提供，不能从磁盘计划继承授权。
    pub authorize_legacy: bool,
}

#[derive(Debug, Serialize)]
pub struct DeleteRequest {
    pub id: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub state: &'static str,
}

#[derive(Debug, Serialize)]
pub struct ExecutionReport {
    pub mode: &'static str,
    pub runtime_id: String,
    pub plan_id: String,
    pub selected_count: usize,
    pub count: usize,
    pub bytes: u64,
    pub requests: Vec<DeleteRequest>,
    pub errors: Vec<Issue>,
    pub complete: bool,
    pub directories_removed: usize,
    pub bytes_meaning: &'static str,
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}
fn same(a: &Path, b: &Path) -> bool {
    key(a) == key(b)
}
fn within(path: &Path, root: &Path) -> bool {
    same(path, root) || key(path).starts_with(&(key(root) + "\\"))
}
fn overlaps(a: &Path, b: &Path) -> bool {
    within(a, b) || within(b, a)
}

#[derive(Deserialize)]
struct RawConfig {
    db_dir: String,
    keys_file: Option<String>,
    key_store: Option<String>,
    decrypted_dir: Option<String>,
    #[serde(default)]
    wechat_process: String,
}

/// 固定所选配置，严格预检 db_dir，再共用 RuntimeContext 身份实现。
pub fn selected_runtime(config_path: &Path, runtime_root: &Path) -> Result<RuntimeContext> {
    let config_path = absolute(config_path)?;
    let root = absolute(runtime_root)?;
    let mut config_file = PinnedFile::open(&config_path, false)?;
    let raw: RawConfig = serde_json::from_slice(&config_file.json_bytes()?)
        .context("配置必须明确包含 db_dir，不能在清理时猜测账号")?;
    ensure!(!raw.db_dir.trim().is_empty(), "配置 db_dir 不能为空");
    let base = config_path.parent().context("配置路径缺少父目录")?;
    let resolve = |value: &str| -> Result<PathBuf> {
        ensure!(!value.trim().is_empty(), "配置路径不能为空");
        let path = PathBuf::from(value);
        absolute(&if path.is_absolute() {
            path
        } else {
            base.join(path)
        })
    };
    let config = Config {
        db_dir: resolve(&raw.db_dir)?,
        keys_file: resolve(raw.keys_file.as_deref().unwrap_or("all_keys.json"))?,
        key_store: raw.key_store.as_deref().map(resolve).transpose()?,
        decrypted_dir: resolve(raw.decrypted_dir.as_deref().unwrap_or("decrypted"))?,
        wechat_process: raw.wechat_process,
    };
    let runtime = RuntimeContext::from_config(config_path, config, root)?;
    absolute(&runtime.directory)?;
    Ok(runtime)
}

struct Protected {
    path: PathBuf,
    directory: bool,
    current_key: bool,
}
struct Bound {
    runtime: RuntimeContext,
    config_pin: PinnedFile,
    roots: BTreeMap<Category, PathBuf>,
    protected: Vec<Protected>,
}

impl Bound {
    fn new(runtime: &RuntimeContext) -> Result<Self> {
        ensure!(valid_hash(&runtime.id), "runtime 标识无效");
        let config_pin = PinnedFile::open(&absolute(&runtime.config_path)?, false)?;
        let fixed = selected_runtime(&runtime.config_path, &runtime.root)?;
        ensure!(
            fixed.id == runtime.id && same(&fixed.directory, &absolute(&runtime.directory)?),
            "runtime 身份与显式配置不符"
        );
        for (a, b) in [
            (&fixed.config.db_dir, &runtime.config.db_dir),
            (&fixed.config.keys_file, &runtime.config.keys_file),
            (&fixed.config.decrypted_dir, &runtime.config.decrypted_dir),
        ] {
            ensure!(same(a, &absolute(b)?), "固定账号后配置路径变化");
        }
        ensure!(
            fixed.config.key_store == runtime.config.key_store,
            "固定账号后密钥存储引用变化"
        );
        let base = fixed.config_path.parent().context("配置路径缺少父目录")?;
        let roots = [
            (Category::RuntimeCache, fixed.cache_dir()),
            (
                Category::DecryptedDatabases,
                fixed.config.decrypted_dir.clone(),
            ),
            (Category::VoiceWavCache, base.join("decoded_voices")),
            (Category::DecodedImages, base.join("decoded_images")),
            (Category::ExportedChats, base.join("exported_chats")),
            (Category::LegacyExports, base.join("exports")),
            (
                Category::RuntimeWebExports,
                fixed.directory.join("web-output"),
            ),
            (Category::KeyCache, fixed.config.keys_file.clone()),
        ]
        .into_iter()
        .map(|(category, path)| Ok((category, absolute(&path)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
        let mut protected = vec![
            Protected {
                path: fixed.config.db_dir.clone(),
                directory: true,
                current_key: false,
            },
            Protected {
                path: fixed.config_path.clone(),
                directory: false,
                current_key: false,
            },
            Protected {
                path: fixed.config.keys_file.clone(),
                directory: false,
                current_key: true,
            },
        ];
        if let Some(path) = &fixed.config.key_store {
            protected.push(Protected {
                path: path.clone(),
                directory: false,
                current_key: false,
            });
        }
        protected.push(Protected {
            path: base.join("account_key.dpapi"),
            directory: false,
            current_key: false,
        });
        // 原始 db_storage 的同账号附件也属于输入，不允许当成缓存接管。
        if fixed
            .config
            .db_dir
            .file_name()
            .is_some_and(|s| s.eq_ignore_ascii_case("db_storage"))
        {
            if let Some(parent) = fixed.config.db_dir.parent() {
                protected.push(Protected {
                    path: parent.into(),
                    directory: true,
                    current_key: false,
                });
            }
        }
        let project = Path::new(env!("CARGO_MANIFEST_DIR"));
        for name in ["src", "vendor", ".git"] {
            protected.push(Protected {
                path: absolute(&project.join(name))?,
                directory: true,
                current_key: false,
            });
        }
        for name in ["Cargo.toml", "Cargo.lock", "AGENTS.md"] {
            protected.push(Protected {
                path: absolute(&project.join(name))?,
                directory: false,
                current_key: false,
            });
        }
        protected.push(Protected {
            path: absolute(&std::env::current_exe()?)?,
            directory: false,
            current_key: false,
        });
        Ok(Self {
            runtime: fixed,
            config_pin,
            roots,
            protected,
        })
    }

    fn root(&self, category: Category) -> &Path {
        &self.roots[&category]
    }

    fn allowed_root(&self, category: Category) -> Result<()> {
        ensure!(
            category != Category::KeyCache,
            "密钥默认保护，仅独立授权入口可选择当前 keys_file"
        );
        let root = self.root(category);
        ensure!(
            root.parent().is_some() && !same(root, &self.runtime.directory),
            "不能清理账号根目录"
        );
        ensure!(
            !self.protected.iter().any(|p| overlaps(root, &p.path)),
            "分类目录与原始库、配置、密钥或源码双向重叠"
        );
        let accounts = self.runtime.root.join("accounts");
        ensure!(
            !overlaps(root, &accounts) || within(root, &self.runtime.directory),
            "分类目录覆盖其它账号 runtime"
        );
        for component in root.components() {
            if let Component::Normal(name) = component {
                let name = name.to_string_lossy().to_lowercase();
                ensure!(
                    !matches!(name.as_str(), "xwechat" | "xwechat_files" | "db_storage"),
                    "拒绝原始微信数据目录及其子目录"
                );
            }
        }
        Ok(())
    }

    fn allowed_key(&self, path: &Path) -> Result<()> {
        ensure!(
            same(path, &self.runtime.config.keys_file),
            "只能选择当前配置绑定的 keys_file"
        );
        ensure!(
            !self
                .protected
                .iter()
                .any(|p| !p.current_key && overlaps(path, &p.path)),
            "密钥路径与配置、源码或原始输入重叠"
        );
        let accounts = self.runtime.root.join("accounts");
        ensure!(
            !overlaps(path, &accounts) || within(path, &self.runtime.directory),
            "密钥路径属于其它账号 runtime"
        );
        // 仅这个已绑定文件的文件名有独立规则，父目录及通用候选规则均不放宽。
        denied_components(path.parent().context("密钥路径缺少父目录")?)?;
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("密钥文件名无效")?
            .to_lowercase();
        let stem = name.split('.').next().unwrap_or("");
        ensure!(
            path.extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("json"))
                && !name.starts_with('_')
                && !matches!(
                    stem,
                    "config"
                        | "credentials"
                        | "secrets"
                        | "image_key"
                        | "wxwork_keys"
                        | "account_key"
                ),
            "仅支持配置绑定的 JSON 数据库密钥缓存；DPAPI、配置及其它密钥始终保护"
        );
        Ok(())
    }

    fn key_guard(&self, path: &Path) -> Result<HostOutputGuard> {
        self.allowed_key(path)?;
        HostOutputGuard::new(path.parent().context("密钥路径缺少父目录")?)
    }

    fn guard(&self, category: Category) -> Result<HostOutputGuard> {
        self.allowed_root(category)?;
        let mut guard = HostOutputGuard::new(self.root(category))?;
        for protected in &self.protected {
            if protected.directory {
                guard.protect_future(&protected.path)?;
            } else {
                match fs::symlink_metadata(&protected.path) {
                    Ok(_) => guard.pin_input(&protected.path)?,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        let parent = protected.path.parent().context("保护路径缺少父目录")?;
                        match fs::symlink_metadata(parent) {
                            Ok(_) => guard.protect(&protected.path)?,
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                                guard.protect_future(parent)?
                            }
                            Err(error) => return Err(error.into()),
                        }
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(guard)
    }

    fn classify_inventory(&self, path: &Path) -> Result<Category> {
        self.roots
            .iter()
            .filter(|(category, root)| {
                **category != Category::KeyCache && within(path, root) && !same(path, root)
            })
            .max_by_key(|(_, root)| root.as_os_str().len())
            .map(|(category, _)| *category)
            .context("原生清单必须位于固定分类目录下，不能提供任意扫描根")
    }

    fn control_path(&self, path: &Path) -> Result<PathBuf> {
        let path = absolute(path)?;
        ensure!(
            path.extension()
                .is_some_and(|s| s.eq_ignore_ascii_case("json")),
            "计划或接管清单必须是 JSON 文件"
        );
        ensure!(
            !self.protected.iter().any(|p| overlaps(&path, &p.path)),
            "控制文件不得是配置、密钥、源码或原始输入"
        );
        ensure!(
            !self
                .roots
                .iter()
                .any(|(category, root)| *category != Category::KeyCache && within(&path, root)),
            "控制文件必须位于所有清理分类之外"
        );
        denied_components(&path)?;
        Ok(path)
    }
}

fn relative(path: &Path) -> Result<PathBuf> {
    ensure!(
        !path.as_os_str().is_empty() && !path.is_absolute(),
        "清单成员必须是非空相对路径"
    );
    let value = path.to_str().context("清单成员路径无效")?;
    ensure!(
        !value
            .split(['/', '\\'])
            .any(|s| s == "." || s == ".." || s.is_empty()),
        "清单成员路径不允许跳转或空分量"
    );
    ensure!(path.components().count() <= 64, "清单路径过深");
    for component in path.components() {
        match component {
            Component::Normal(name) => safe_component(name.to_str().context("清单路径无效")?)?,
            _ => anyhow::bail!("清单路径必须只有普通文件名分量"),
        }
    }
    Ok(path.into())
}

fn denied_components(path: &Path) -> Result<()> {
    for component in path.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        let name = name.to_string_lossy().to_lowercase();
        let stem = name.split('.').next().unwrap_or("");
        ensure!(
            !matches!(
                name.as_str(),
                "src"
                    | "source"
                    | "vendor"
                    | ".git"
                    | "xwechat"
                    | "xwechat_files"
                    | "db_storage"
                    | "config"
                    | "keys"
                    | ".env"
                    | "_mtimes.json"
                    | "_directory_export.json"
                    | "_cleanup_inventory.json"
                    | "_source_binding.json"
                    | "_export_index.json"
            ) && !name.starts_with("all_keys")
                && !matches!(
                    stem,
                    "config"
                        | "key"
                        | "keys"
                        | "credentials"
                        | "secrets"
                        | "image_key"
                        | "wxwork_keys"
                )
                && !name.ends_with("-wal")
                && !name.ends_with("-shm")
                && !name.ends_with("-journal"),
            "拒绝配置、密钥、原始库、控制清单或源码路径"
        );
    }
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    ensure!(
        !matches!(
            extension.as_str(),
            "rs" | "py"
                | "js"
                | "ts"
                | "toml"
                | "lock"
                | "exe"
                | "dll"
                | "sys"
                | "key"
                | "pem"
                | "ini"
                | "ps1"
                | "bat"
                | "cmd"
                | "vbs"
                | "lnk"
                | "url"
        ),
        "拒绝密钥、配置、程序或链接文件类型"
    );
    Ok(())
}

fn known_file(category: Category, path: &Path, prefix: &[u8; 16]) -> Result<()> {
    denied_components(path)?;
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    match category {
        Category::RuntimeCache | Category::DecryptedDatabases => {
            ensure!(
                extension == "db" && prefix == b"SQLite format 3\0",
                "不删除无法识别的解密 SQLite 文件"
            );
        }
        Category::VoiceWavCache => {
            ensure!(
                extension == "wav" && &prefix[..4] == b"RIFF" && &prefix[8..12] == b"WAVE",
                "不删除无法识别的 WAV 文件"
            );
        }
        Category::DecodedImages => {
            let valid = match extension.as_str() {
                "png" => &prefix[..8] == b"\x89PNG\r\n\x1a\n",
                "jpg" | "jpeg" => &prefix[..3] == b"\xff\xd8\xff",
                "gif" => &prefix[..6] == b"GIF87a" || &prefix[..6] == b"GIF89a",
                "webp" => &prefix[..4] == b"RIFF" && &prefix[8..12] == b"WEBP",
                "bmp" => &prefix[..2] == b"BM",
                "tif" | "tiff" => &prefix[..4] == b"II*\0" || &prefix[..4] == b"MM\0*",
                _ => false,
            };
            ensure!(valid, "不删除无法识别的图片文件");
        }
        Category::ExportedChats | Category::LegacyExports | Category::RuntimeWebExports => {
            ensure!(
                matches!(
                    extension.as_str(),
                    "json"
                        | "csv"
                        | "html"
                        | "htm"
                        | "md"
                        | "txt"
                        | "png"
                        | "jpg"
                        | "jpeg"
                        | "gif"
                        | "webp"
                        | "bmp"
                        | "tif"
                        | "tiff"
                        | "wav"
                        | "mp3"
                        | "mp4"
                        | "pdf"
                        | "silk"
                        | "aac"
                        | "ogg"
                        | "opus"
                ) || path.file_name().is_some_and(|s| s == ".info"),
                "不删除未知导出文件类型"
            );
        }
        Category::KeyCache => anyhow::bail!("普通候选入口不接受密钥；必须独立授权当前 keys_file"),
    }
    Ok(())
}

#[derive(Deserialize)]
struct CacheEntry {
    path: String,
    db_mt: u64,
    wal_mt: u64,
}
#[derive(Deserialize)]
struct NativeInventory {
    version: u32,
    runtime_id: String,
    files: BTreeMap<String, String>,
}

struct UsageBudget {
    entries: usize,
    started: Instant,
    files: BTreeSet<(u32, u64)>,
    bytes: u64,
}

fn usage_error(
    usage: &mut DiskUsage,
    category: Category,
    path: &Path,
    error: impl std::fmt::Display,
) {
    usage.partial = true;
    usage.skipped_count += 1;
    if usage.errors.len() < MAX_INPUTS {
        usage.errors.push(Issue {
            category: Some(category),
            path: path.into(),
            error: error.to_string(),
        });
    }
}

/// 仅在固定根下逐层固定目录后枚举；普通文件只打开 READ_ATTRIBUTES 句柄。
fn usage_tree(
    bound: &Bound,
    category: Category,
    path: &Path,
    depth: usize,
    usage: &mut DiskUsage,
    budget: &mut UsageBudget,
) -> Result<()> {
    ensure!(depth <= MAX_USAGE_DEPTH, "空间统计达到目录深度限制");
    ensure!(
        budget.entries < MAX_USAGE_ENTRIES && budget.started.elapsed() < MAX_USAGE_TIME,
        "空间统计达到条目或时间限制"
    );
    let guard = HostOutputGuard::new(path)?;
    let before = fs::symlink_metadata(path)?.last_write_time();
    for entry in fs::read_dir(path)? {
        ensure!(
            budget.entries < MAX_USAGE_ENTRIES && budget.started.elapsed() < MAX_USAGE_TIME,
            "空间统计达到条目或时间限制"
        );
        budget.entries += 1;
        usage.entries_visited += 1;
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                usage_error(usage, category, path, error);
                continue;
            }
        };
        let child = entry.path();
        let result = (|| -> Result<()> {
            safe_component(entry.file_name().to_str().context("无效目录项名称")?)?;
            ensure!(
                !bound.protected.iter().any(|p| overlaps(&child, &p.path)),
                "统计跳过受保护输入"
            );
            let accounts = bound.runtime.root.join("accounts");
            ensure!(
                !overlaps(&child, &accounts) || within(&child, &bound.runtime.directory),
                "统计跳过其它账号目录"
            );
            let metadata = fs::symlink_metadata(&child)?;
            ensure!(
                metadata.file_attributes() & 0x400 == 0 && !metadata.file_type().is_symlink(),
                "统计不跟随链接或重解析点"
            );
            if metadata.is_dir() {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                ensure!(
                    !matches!(
                        name.as_str(),
                        "src"
                            | "source"
                            | "vendor"
                            | ".git"
                            | "xwechat"
                            | "xwechat_files"
                            | "db_storage"
                            | "accounts"
                    ),
                    "统计跳过源码或账号来源目录"
                );
                usage_tree(bound, category, &child, depth + 1, usage, budget)?;
            } else {
                ensure!(metadata.is_file(), "统计跳过特殊文件");
                let (volume, index, bytes) = handles::metadata_only_file(&child)?;
                usage.bytes = usage
                    .bytes
                    .checked_add(bytes)
                    .context("空间统计字节数溢出")?;
                usage.count += 1;
                if budget.files.insert((volume, index)) {
                    budget.bytes = budget
                        .bytes
                        .checked_add(bytes)
                        .context("空间统计总字节数溢出")?;
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            usage_error(usage, category, &child, error);
        }
    }
    guard.verify()?;
    ensure!(
        before == fs::symlink_metadata(path)?.last_write_time(),
        "遍历期间目录变化，空间统计为不完整观察"
    );
    Ok(())
}

struct Collector<'a> {
    bound: &'a Bound,
    plan: Plan,
    guards: BTreeMap<Category, HostOutputGuard>,
    evidence_pins: Vec<PinnedFile>,
    paths: BTreeSet<String>,
}

impl<'a> Collector<'a> {
    fn usage(&mut self) {
        let mut budget = UsageBudget {
            entries: 0,
            started: Instant::now(),
            files: BTreeSet::new(),
            bytes: 0,
        };
        for category in [
            Category::DecryptedDatabases,
            Category::VoiceWavCache,
            Category::DecodedImages,
            Category::ExportedChats,
            Category::LegacyExports,
        ] {
            let root = self.bound.root(category).to_path_buf();
            let mut usage = DiskUsage::default();
            let result = (|| -> Result<()> {
                self.bound.allowed_root(category)?;
                // 固定现有祖先再探测根目录，缺失不创建，也不转向其它路径。
                let root = absolute(&root)?;
                match fs::symlink_metadata(&root) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error.into()),
                    Ok(_) => usage_tree(self.bound, category, &root, 0, &mut usage, &mut budget),
                }
            })();
            if let Err(error) = result {
                usage_error(&mut usage, category, &root, error);
            }
            self.plan.disk_usage.partial |= usage.partial;
            self.plan.disk_usage.skipped_count += usage.skipped_count;
            self.plan
                .disk_usage
                .errors
                .extend(usage.errors.iter().cloned());
            let summary = self.category_mut(category);
            summary.unlisted_files_inspected = true;
            summary.disk_usage = Some(usage);
        }
        // 固定分类可能嵌套：分类值独立统计，总值按文件身份去重。
        self.plan.disk_usage.count = budget.files.len();
        self.plan.disk_usage.bytes = budget.bytes;
        self.plan.disk_usage.entries_visited = budget.entries;
    }

    fn error(&mut self, category: Option<Category>, path: &Path, error: impl std::fmt::Display) {
        self.plan.errors.push(Issue {
            category,
            path: path.into(),
            error: error.to_string(),
        });
    }

    fn category_mut(&mut self, category: Category) -> &mut CategoryStatus {
        self.plan
            .categories
            .iter_mut()
            .find(|entry| entry.id == category)
            .expect("固定分类存在")
    }

    fn root_guard(&mut self, category: Category) -> Result<()> {
        if !self.guards.contains_key(&category) {
            self.guards.insert(category, self.bound.guard(category)?);
        }
        self.guards[&category].verify()
    }

    fn evidence<T: serde::de::DeserializeOwned>(&mut self, path: &Path, kind: &str) -> Result<T> {
        let mut pin = PinnedFile::open(path, false)?;
        let value =
            serde_json::from_slice(&pin.json_bytes()?).context("来源清单 JSON 格式不正确")?;
        self.plan.evidence.push(Evidence {
            path: path.into(),
            kind: kind.into(),
            fingerprint: pin.snapshot()?,
            directories: pin.directories()?,
        });
        self.evidence_pins.push(pin);
        Ok(value)
    }

    fn candidate(
        &mut self,
        category: Category,
        path: &Path,
        origin: &str,
        evidence: &Path,
        expected: Option<&str>,
    ) -> Result<()> {
        ensure!(self.plan.files.len() < MAX_FILES, "计划文件数量超过限制");
        self.bound.allowed_root(category)?;
        let path = absolute(path)?;
        ensure!(
            within(&path, self.bound.root(category)) && !same(&path, self.bound.root(category)),
            "候选文件越过固定分类目录"
        );
        denied_components(&path)?;
        ensure!(
            !self
                .bound
                .protected
                .iter()
                .any(|p| overlaps(&path, &p.path)),
            "候选文件与受保护输入重叠"
        );
        self.root_guard(category)?;
        ensure!(
            self.paths.insert(key(&path)),
            "同一文件不能从多个来源重复接管"
        );
        let mut pin = match PinnedFile::open(&path, false) {
            Ok(pin) => pin,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                self.category_mut(category).missing_count += 1;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        known_file(category, &path, &pin.prefix()?)?;
        let fingerprint = pin.snapshot()?;
        if let Some(expected) = expected {
            ensure!(
                valid_hash(expected) && fingerprint.sha256 == expected,
                "文件与绑定清单中的 SHA-256 不符"
            );
        }
        self.record_candidate(
            category,
            path,
            origin,
            evidence,
            fingerprint,
            pin.directories()?,
        )
    }

    fn key_candidate(&mut self) -> Result<()> {
        ensure!(self.plan.key_removal_authorized, "密钥候选缺少本次明确授权");
        let path = absolute(&self.bound.runtime.config.keys_file)?;
        self.bound.allowed_key(&path)?;
        let guard = self.bound.key_guard(&path)?;
        let mut pin = match PinnedFile::open(&path, false) {
            Ok(pin) => pin,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                self.category_mut(Category::KeyCache).missing_count += 1;
                return Ok(());
            }
            Err(error) => return Err(error),
        };
        ensure!(
            handles::metadata_only_file(&path)?.2 <= MAX_JSON_BYTES,
            "密钥缓存超过读取上限"
        );
        let fingerprint = pin.snapshot()?;
        guard.verify()?;
        let evidence = self.bound.runtime.config_path.clone();
        self.record_candidate(
            Category::KeyCache,
            path,
            "explicit-current-config-key-only",
            &evidence,
            fingerprint,
            pin.directories()?,
        )
    }

    fn record_candidate(
        &mut self,
        category: Category,
        path: PathBuf,
        origin: &str,
        evidence: &Path,
        fingerprint: Fingerprint,
        directories: Vec<DirectoryIdentity>,
    ) -> Result<()> {
        let bytes = self
            .plan
            .bytes
            .checked_add(fingerprint.bytes)
            .context("计划字节数溢出")?;
        ensure!(bytes <= MAX_TOTAL_BYTES, "计划总读取字节数超过限制");
        let id = hash(&serde_json::to_vec(&(
            self.bound.runtime.id.as_str(),
            category.id(),
            key(&path),
            &fingerprint,
        ))?);
        self.plan.files.push(PlannedFile {
            id,
            category,
            path,
            origin: origin.into(),
            evidence: evidence.into(),
            fingerprint: fingerprint.clone(),
            directories,
        });
        self.plan.bytes = bytes;
        self.plan.count += 1;
        let summary = self.category_mut(category);
        summary.inspected = true;
        summary.count += 1;
        summary.bytes += fingerprint.bytes;
        Ok(())
    }

    fn cache(&mut self) -> Result<()> {
        let category = Category::RuntimeCache;
        self.bound.allowed_root(category)?;
        let path = self.bound.runtime.mtime_file();
        // 缓存从未创建时只报告空计划，绝不创建 runtime 目录。
        match fs::symlink_metadata(self.bound.root(category)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
            Ok(_) => self.root_guard(category)?,
        }
        match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
            Ok(_) => (),
        }
        let entries: BTreeMap<String, CacheEntry> = self.evidence(&path, "runtime-cache-index")?;
        ensure!(entries.len() <= MAX_FILES, "缓存索引过大");
        self.category_mut(category).inspected = true;
        for (source, entry) in entries {
            let result = (|| -> Result<()> {
                let rel = relative(Path::new(&source.replace('\\', "/")))?;
                ensure!(
                    rel.extension()
                        .is_some_and(|s| s.eq_ignore_ascii_case("db")),
                    "缓存索引原始键不是数据库"
                );
                let expected = self
                    .bound
                    .runtime
                    .cache_dir()
                    .join(format!("{:x}.db", md5::compute(source.as_bytes())));
                ensure!(
                    same(&absolute(Path::new(&entry.path))?, &expected),
                    "缓存索引指向非标准位置，拒绝跟随"
                );
                // 时间戳属于原始库索引，不读取原始库或其密钥进行清理。
                let _ = (entry.db_mt, entry.wal_mt);
                self.candidate(category, &expected, "runtime-index-bound", &path, None)
            })();
            if let Err(error) = result {
                self.error(Some(category), &path, error);
            }
        }
        Ok(())
    }

    fn inventory(&mut self, path: &Path) -> Result<()> {
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .context("原生清单缺少文件名")?;
        ensure!(
            matches!(name, "_directory_export.json" | "_cleanup_inventory.json"),
            "不支持的原生绑定清单名称"
        );
        let category = self.bound.classify_inventory(path)?;
        ensure!(
            !matches!(category, Category::RuntimeCache | Category::KeyCache),
            "此分类不接收原生产物清单"
        );
        self.root_guard(category)?;
        let inventory: NativeInventory = self.evidence(path, "native-runtime-inventory")?;
        ensure!(
            inventory.version == VERSION && inventory.runtime_id == self.bound.runtime.id,
            "原生产物清单不属于当前 runtime"
        );
        ensure!(inventory.files.len() <= MAX_FILES, "原生产物清单过大");
        self.category_mut(category).inspected = true;
        let parent = path.parent().context("原生清单缺少父目录")?;
        for (name, digest) in inventory.files {
            let result = (|| {
                let rel = relative(Path::new(&name))?;
                self.candidate(
                    category,
                    &parent.join(rel),
                    "native-runtime-hash-bound",
                    path,
                    Some(&digest),
                )
            })();
            if let Err(error) = result {
                self.error(Some(category), &parent.join(name), error);
            }
        }
        Ok(())
    }

    fn adoption(&mut self, path: &Path) -> Result<()> {
        let data: AdoptionManifest = self.evidence(path, "explicit-legacy-adoption")?;
        ensure!(
            data.version == VERSION && data.runtime_id == self.bound.runtime.id,
            "旧文件接管清单不属于当前 runtime"
        );
        ensure!(
            data.authorization == "explicit-files-only",
            "接管清单必须声明 explicit-files-only"
        );
        ensure!(data.files.len() <= MAX_FILES, "接管清单过大");
        for entry in data.files {
            let root = self.bound.root(entry.category).to_path_buf();
            let result = (|| {
                ensure!(
                    entry.category.legacy(),
                    "只有旧脚本的五个非密钥分类可显式接管"
                );
                let rel = relative(&entry.relative_path)?;
                self.candidate(
                    entry.category,
                    &root.join(rel),
                    "legacy-user-authorized-origin-unverified",
                    path,
                    entry.sha256.as_deref(),
                )
            })();
            if let Err(error) = result {
                self.error(
                    Some(entry.category),
                    &root.join(&entry.relative_path),
                    error,
                );
            }
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Plan> {
        for guard in self.guards.values() {
            guard.verify()?;
        }
        // 来源清单始终保持只读句柄，结束前再次校验内容与祖先身份。
        for (pin, evidence) in self
            .evidence_pins
            .iter_mut()
            .zip(self.plan.evidence.iter().skip(1))
        {
            ensure!(
                pin.snapshot()? == evidence.fingerprint,
                "生成计划期间来源清单变化"
            );
            pin.verify_directories(&evidence.directories)?;
        }
        self.plan.files.sort_by(|a, b| a.id.cmp(&b.id));
        self.plan.evidence.sort_by(|a, b| a.path.cmp(&b.path));
        self.plan.plan_id = self.plan.seal()?;
        Ok(self.plan)
    }
}

/// 默认状态只读：统计五个固定旧目录元数据；候选仍只来自当前 runtime 缓存索引。
#[cfg(test)]
pub fn status_for(runtime: &RuntimeContext) -> Result<Plan> {
    plan_for(runtime, &PlanInputs::default(), None)
}

/// authorize_legacy_account 必须来自本次显式授权，且逐字等于当前 runtime ID。
#[cfg(test)]
pub fn plan_for(
    runtime: &RuntimeContext,
    inputs: &PlanInputs,
    authorize_legacy_account: Option<&str>,
) -> Result<Plan> {
    plan_with_key_removal_for(runtime, inputs, authorize_legacy_account, None)
}

/// 危险选项独立于常规/Web API；密钥授权必须来自本次调用，不能从清单继承。
pub fn plan_with_key_removal_for(
    runtime: &RuntimeContext,
    inputs: &PlanInputs,
    authorize_legacy_account: Option<&str>,
    authorize_key_account: Option<&str>,
) -> Result<Plan> {
    ensure!(
        inputs.native_inventories.len() <= MAX_INPUTS,
        "原生清单数量超过限制"
    );
    if let Some(account) = authorize_key_account {
        ensure!(account == runtime.id, "移除密钥需逐字确认当前 runtime ID");
        ensure!(
            inputs.native_inventories.is_empty()
                && inputs.adoption_manifest.is_none()
                && authorize_legacy_account.is_none(),
            "密钥必须单独生成计划，不能混入普通或旧产物清理"
        );
    }
    if inputs.adoption_manifest.is_some() {
        ensure!(
            authorize_legacy_account == Some(runtime.id.as_str()),
            "读取旧文件需显式接管授权并确认当前账号 ID"
        );
    }
    let mut bound = Bound::new(runtime)?;
    let config_evidence = Evidence {
        path: bound.runtime.config_path.clone(),
        kind: "fixed-runtime-config".into(),
        fingerprint: bound.config_pin.snapshot()?,
        directories: bound.config_pin.directories()?,
    };
    let mut normalized = PlanInputs {
        native_inventories: inputs
            .native_inventories
            .iter()
            .map(|p| absolute(p))
            .collect::<Result<Vec<_>>>()?,
        adoption_manifest: inputs
            .adoption_manifest
            .as_ref()
            .map(|p| bound.control_path(p))
            .transpose()?,
    };
    normalized.native_inventories.sort();
    ensure!(
        normalized
            .native_inventories
            .iter()
            .map(|p| key(p))
            .collect::<BTreeSet<_>>()
            .len()
            == normalized.native_inventories.len(),
        "原生清单不可重复"
    );
    let categories = bound
        .roots
        .iter()
        .map(|(id, path)| CategoryStatus {
            id: *id,
            label: id.label().into(),
            path: path.clone(),
            policy: match if *id == Category::KeyCache && authorize_key_account.is_some() {
                bound.allowed_key(path)
            } else {
                bound.allowed_root(*id)
            } {
                Err(error) => format!("protected: {error}"),
                Ok(_) if *id == Category::KeyCache => {
                    "explicit-current-key-only; execution-reauthorization-required".into()
                }
                Ok(_) if *id == Category::RuntimeCache => {
                    "runtime-index-only; unlisted-files-preserved".into()
                }
                Ok(_) if *id == Category::RuntimeWebExports => {
                    "explicit-native-runtime-inventory-required".into()
                }
                Ok(_) => "native-inventory-or-explicit-legacy-adoption-required".into(),
            },
            inspected: false,
            count: 0,
            bytes: 0,
            missing_count: 0,
            unlisted_files_inspected: false,
            disk_usage: None,
        })
        .collect();
    let mut collector = Collector {
        bound: &bound,
        guards: BTreeMap::new(),
        evidence_pins: Vec::new(),
        paths: BTreeSet::new(),
        plan: Plan {
            version: VERSION,
            mode: "dry-run".into(),
            runtime_id: bound.runtime.id.clone(),
            runtime_directory: bound.runtime.directory.clone(),
            config_path: bound.runtime.config_path.clone(),
            policy: POLICY.into(),
            inputs: normalized.clone(),
            key_removal_authorized: authorize_key_account.is_some(),
            categories,
            evidence: vec![config_evidence.clone()],
            files: Vec::new(),
            count: 0,
            bytes: 0,
            disk_usage: DiskUsage::default(),
            errors: Vec::new(),
            plan_id: String::new(),
        },
    };
    collector.usage();
    if authorize_key_account.is_some() {
        if let Err(error) = collector.key_candidate() {
            collector.error(
                Some(Category::KeyCache),
                &bound.runtime.config.keys_file,
                error,
            );
        }
    } else if let Err(error) = collector.cache() {
        collector.error(
            Some(Category::RuntimeCache),
            &bound.runtime.cache_dir(),
            error,
        );
    }
    for path in &normalized.native_inventories {
        if let Err(error) = collector.inventory(path) {
            collector.error(None, path, error);
        }
    }
    if let Some(path) = &normalized.adoption_manifest {
        if let Err(error) = collector.adoption(path) {
            collector.error(None, path, error);
        }
    }
    let plan = collector.finish()?;
    ensure!(
        bound.config_pin.snapshot()? == config_evidence.fingerprint,
        "生成计划期间配置变化"
    );
    Ok(plan)
}

pub fn read_plan_for(runtime: &RuntimeContext, path: &Path) -> Result<Plan> {
    let bound = Bound::new(runtime)?;
    let path = bound.control_path(path)?;
    let mut pin = PinnedFile::open(&path, false)?;
    let plan: Plan = serde_json::from_slice(&pin.json_bytes()?).context("无效清理计划 JSON")?;
    ensure!(
        plan.version == VERSION && plan.runtime_id == bound.runtime.id && plan.policy == POLICY,
        "计划版本、策略或账号不匹配"
    );
    ensure!(
        plan.files.len() <= MAX_FILES && plan.plan_id == plan.seal()?,
        "计划校验失败"
    );
    Ok(plan)
}

/// 只创建一个显式计划文件，不创建父目录、不覆盖已有文件，也不调用清理执行器。
pub fn write_plan_for(runtime: &RuntimeContext, plan: &Plan, path: &Path) -> Result<()> {
    let bound = Bound::new(runtime)?;
    ensure!(
        plan.runtime_id == bound.runtime.id && plan.plan_id == plan.seal()?,
        "计划与当前账号不符"
    );
    let path = bound.control_path(path)?;
    let bytes = serde_json::to_vec_pretty(plan)?;
    ensure!(
        bytes.len() as u64 <= MAX_JSON_BYTES,
        "计划 JSON 超过输出上限"
    );
    let guard = HostOutputGuard::new(path.parent().context("计划路径缺少父目录")?)?;
    guard.verify_replaceable_file(&path)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .custom_flags(0x00200000)
        .open(&path)
        .context("计划文件必须是不存在的新文件，父目录必须已存在")?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    guard.verify()?;
    Ok(())
}

/// 先核验整份计划，再固定所有选中文件；预检失败时一个文件也不删除。
#[cfg(test)]
pub fn execute_for(
    runtime: &RuntimeContext,
    plan: &Plan,
    options: &ExecuteOptions,
) -> Result<ExecutionReport> {
    execute_with_key_removal_for(runtime, plan, options, None)
}

/// 密钥计划执行必须单独重授权限；普通 execute_for 永远不能执行密钥计划。
pub fn execute_with_key_removal_for(
    runtime: &RuntimeContext,
    plan: &Plan,
    options: &ExecuteOptions,
    authorize_key_account: Option<&str>,
) -> Result<ExecutionReport> {
    ensure!(
        options.confirm_account == runtime.id,
        "确认账号 ID 必须与固定 runtime 完全一致"
    );
    ensure!(
        !options.select.is_empty() && options.select.len() <= MAX_SELECTED,
        "必须精确选择 1 至 2048 个文件 ID"
    );
    ensure!(
        options.select.iter().all(|id| valid_hash(id)),
        "只接受完整文件 ID，不支持分类、序号或通配符"
    );
    let selected: BTreeSet<_> = options.select.iter().cloned().collect();
    ensure!(selected.len() == options.select.len(), "文件 ID 不得重复");
    ensure!(
        plan.version == VERSION
            && plan.policy == POLICY
            && plan.runtime_id == runtime.id
            && plan.plan_id == plan.seal()?,
        "计划校验或账号绑定失败"
    );
    ensure!(
        plan.errors.is_empty(),
        "计划包含错误，不允许执行部分可信计划"
    );
    ensure!(
        plan.inputs.adoption_manifest.is_none() || options.authorize_legacy,
        "执行旧产物清理须再次明确授权接管"
    );
    ensure!(
        authorize_key_account.is_some() == plan.key_removal_authorized,
        "计划和执行必须分别明确授权密钥移除，不能临时升级普通计划"
    );
    if plan.key_removal_authorized {
        ensure!(
            authorize_key_account == Some(runtime.id.as_str()),
            "执行密钥移除需再次完整确认账号 ID"
        );
        ensure!(
            !options.authorize_legacy
                && selected.len() == 1
                && plan.files.len() == 1
                && plan.files[0].category == Category::KeyCache,
            "密钥只能作为单个精确文件独立执行"
        );
    }
    let bound = Bound::new(runtime)?;
    let (_runtime_guard, _locks) = handles::runtime_locks(&bound.runtime.directory)?;
    let fresh = plan_with_key_removal_for(
        &bound.runtime,
        &plan.inputs,
        options.authorize_legacy.then_some(runtime.id.as_str()),
        authorize_key_account,
    )?;
    ensure!(
        fresh.errors.is_empty() && fresh.plan_id == plan.plan_id,
        "文件、路径、配置或来源清单已变化，必须重新生成并审阅计划"
    );
    let files: Vec<_> = fresh
        .files
        .iter()
        .filter(|file| selected.contains(&file.id))
        .collect();
    ensure!(
        files.len() == selected.len(),
        "选择中包含不属于此计划的文件 ID"
    );
    let mut source_pins = Vec::new();
    for evidence in &fresh.evidence {
        let mut pin = PinnedFile::open(&evidence.path, false)?;
        ensure!(
            pin.snapshot()? == evidence.fingerprint,
            "来源证据在执行预检时变化"
        );
        pin.verify_directories(&evidence.directories)?;
        source_pins.push(pin);
    }
    let mut guards = BTreeMap::new();
    let mut pinned = Vec::new();
    for file in files {
        if let std::collections::btree_map::Entry::Vacant(e) = guards.entry(file.category) {
            let guard = if file.category == Category::KeyCache {
                ensure!(
                    plan.key_removal_authorized
                        && authorize_key_account == Some(runtime.id.as_str()),
                    "密钥执行未获授权"
                );
                bound.key_guard(&file.path)?
            } else {
                bound.guard(file.category)?
            };
            e.insert(guard);
        }
        ensure!(
            !fresh
                .evidence
                .iter()
                .any(|source| same(&source.path, &file.path)),
            "来源清单不能作为删除目标"
        );
        let mut pin = PinnedFile::open(&file.path, true)?;
        if file.category == Category::KeyCache {
            bound.allowed_key(&file.path)?;
        } else {
            known_file(file.category, &file.path, &pin.prefix()?)?;
        }
        ensure!(
            pin.snapshot()? == file.fingerprint,
            "选中文件在执行预检时变化"
        );
        pin.verify_directories(&file.directories)?;
        pinned.push((file, pin));
    }
    for guard in guards.values() {
        guard.verify()?;
    }
    for (pin, source) in source_pins.iter_mut().zip(&fresh.evidence) {
        ensure!(pin.snapshot()? == source.fingerprint, "删除前来源证据变化");
        pin.verify_directories(&source.directories)?;
    }
    let mut report = ExecutionReport {
        mode: "execute", runtime_id: runtime.id.clone(), plan_id: fresh.plan_id.clone(), selected_count: selected.len(),
        count: 0, bytes: 0, requests: Vec::new(), errors: Vec::new(), complete: false, directories_removed: 0,
        bytes_meaning: "verified original file bytes submitted for deletion; physical reclamation may await other readers; all directories retained",
    };
    for (file, pin) in pinned {
        let result = guards[&file.category]
            .verify()
            .and_then(|_| pin.delete(&file.fingerprint));
        match result {
            Ok(()) => {
                report.count += 1;
                report.bytes += file.fingerprint.bytes;
                report.requests.push(DeleteRequest {
                    id: file.id.clone(),
                    path: file.path.clone(),
                    bytes: file.fingerprint.bytes,
                    state: "verified-handle-delete-requested",
                });
            }
            Err(error) => {
                report.errors.push(Issue {
                    category: Some(file.category),
                    path: file.path.clone(),
                    error: error.to_string(),
                });
                // 执行不是整树事务；首个错误后停止，准确保留已经完成的逐文件结果。
                break;
            }
        }
    }
    report.complete = report.count == report.selected_count && report.errors.is_empty();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FAKE_KEY: &[u8] = b"{\"fixture\":\"SYNTHETIC-NOT-A-WECHAT-KEY\"}";

    struct Fixture {
        temp: tempfile::TempDir,
        runtime: RuntimeContext,
    }

    impl Fixture {
        fn new() -> Result<Self> {
            Self::with_key_name("all_keys.json")
        }

        fn with_key_name(key_name: &str) -> Result<Self> {
            let temp = tempfile::Builder::new()
                .prefix("wx-cleanup-test-")
                .tempdir()?;
            let work = temp.path().join("work");
            let input = temp.path().join("input-db");
            fs::create_dir(&work)?;
            fs::create_dir(&input)?;
            fs::write(
                input.join("fixture.db"),
                b"synthetic source, not a database",
            )?;
            fs::write(work.join(key_name), FAKE_KEY)?;
            let config_path = work.join("config.json");
            fs::write(
                &config_path,
                serde_json::to_vec(&json!({
                    "db_dir": input,
                    "keys_file": key_name,
                    "decrypted_dir": "decrypted",
                    "wechat_process": "synthetic-test.exe"
                }))?,
            )?;
            let runtime = selected_runtime(&config_path, &temp.path().join("state"))?;
            Ok(Self { temp, runtime })
        }

        fn work(&self) -> &Path {
            self.runtime.config_path.parent().unwrap()
        }

        fn ready_runtime(&self) -> Result<()> {
            ensure!(
                within(&self.runtime.directory, &absolute(self.temp.path())?),
                "fixture escaped its temporary directory"
            );
            fs::create_dir_all(&self.runtime.directory)?;
            Ok(())
        }

        fn key_plan(&self) -> Result<Plan> {
            plan_with_key_removal_for(
                &self.runtime,
                &PlanInputs::default(),
                None,
                Some(&self.runtime.id),
            )
        }

        fn options(&self, plan: &Plan) -> ExecuteOptions {
            ExecuteOptions {
                select: plan.files.iter().map(|file| file.id.clone()).collect(),
                confirm_account: self.runtime.id.clone(),
                authorize_legacy: false,
            }
        }

        fn inventory(&self, name: &str, bytes: &[u8]) -> Result<PlanInputs> {
            let root = self.work().join("exported_chats");
            fs::create_dir_all(&root)?;
            fs::write(root.join(name), bytes)?;
            let path = root.join("_directory_export.json");
            let files = BTreeMap::from([(name, hash(bytes))]);
            fs::write(
                &path,
                serde_json::to_vec(&json!({
                    "version": VERSION, "runtime_id": self.runtime.id, "files": files
                }))?,
            )?;
            Ok(PlanInputs {
                native_inventories: vec![path],
                adoption_manifest: None,
            })
        }
    }

    #[test]
    fn disk_usage_counts_five_fixed_roots_without_deletion_candidates() -> Result<()> {
        let fixture = Fixture::new()?;
        let directories = [
            "decrypted",
            "decoded_voices",
            "decoded_images",
            "exported_chats",
            "exports",
        ];
        let mut expected_bytes = 0;
        for (index, name) in directories.iter().enumerate() {
            let root = fixture.work().join(name);
            fs::create_dir(&root)?;
            let bytes = vec![0xff; index + 3];
            expected_bytes += bytes.len() as u64;
            fs::write(root.join("unknown.blob"), bytes)?;
        }
        fs::write(fixture.temp.path().join("outside.blob"), b"not counted")?;
        assert!(!fixture.runtime.directory.exists());
        let plan = status_for(&fixture.runtime)?;
        assert!(plan.errors.is_empty(), "{:?}", plan.errors);
        assert!(!plan.disk_usage.partial, "{:?}", plan.disk_usage.errors);
        assert_eq!(
            (plan.disk_usage.count, plan.disk_usage.bytes),
            (5, expected_bytes)
        );
        assert_eq!((plan.count, plan.bytes), (0, 0));
        assert!(plan.files.is_empty());
        assert!(!plan.key_removal_authorized);
        for category in plan
            .categories
            .iter()
            .filter(|category| category.id.legacy())
        {
            assert_eq!(category.disk_usage.as_ref().unwrap().count, 1);
            assert_eq!(category.count, 0);
            assert!(category.unlisted_files_inspected);
        }
        assert!(
            !fixture.runtime.directory.exists(),
            "status must not create runtime or locks"
        );
        assert_eq!(fs::read(&fixture.runtime.config.keys_file)?, FAKE_KEY);
        Ok(())
    }

    #[test]
    fn inventory_execution_preserves_changed_unknown_files() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.ready_runtime()?;
        let content = b"synthetic exported chat";
        let inputs = fixture.inventory("chat.txt", content)?;
        let root = fixture.work().join("exported_chats");
        let unknown = root.join("unknown.blob");
        fs::write(&unknown, b"unknown original")?;
        let plan = plan_for(&fixture.runtime, &inputs, None)?;
        assert!(plan.errors.is_empty(), "{:?}", plan.errors);
        assert!(!plan.disk_usage.partial, "{:?}", plan.disk_usage.errors);
        let inventory_bytes = fs::metadata(&inputs.native_inventories[0])?.len();
        assert_eq!(plan.disk_usage.count, 3);
        assert_eq!(
            plan.disk_usage.bytes,
            content.len() as u64 + inventory_bytes + 16
        );
        assert_eq!((plan.count, plan.bytes), (1, content.len() as u64));
        assert!(same(&plan.files[0].path, &root.join("chat.txt")));

        // Unlisted metadata is observational, not an executable file binding.
        fs::write(&unknown, b"unknown changed after planning")?;
        let report = execute_for(&fixture.runtime, &plan, &fixture.options(&plan))?;
        assert!(report.complete, "{:?}", report.errors);
        assert_eq!((report.count, report.bytes), (1, content.len() as u64));
        assert_eq!(report.directories_removed, 0);
        assert!(!root.join("chat.txt").exists());
        assert_eq!(fs::read(&unknown)?, b"unknown changed after planning");
        assert!(inputs.native_inventories[0].exists());
        assert!(root.is_dir());
        assert_eq!(fs::read(&fixture.runtime.config.keys_file)?, FAKE_KEY);
        Ok(())
    }

    #[test]
    fn unknown_inventory_member_is_not_a_deletion_candidate() -> Result<()> {
        let fixture = Fixture::new()?;
        let inputs = fixture.inventory("unknown.blob", b"synthetic unknown file")?;
        let plan = plan_for(&fixture.runtime, &inputs, None)?;
        assert!(!plan.errors.is_empty());
        assert!(plan.files.is_empty());
        assert_eq!(plan.count, 0);
        assert_eq!(plan.disk_usage.count, 2);
        assert!(fixture.work().join("exported_chats/unknown.blob").exists());
        assert!(!fixture.runtime.directory.exists());
        Ok(())
    }

    #[test]
    fn protected_subtree_makes_usage_partial_without_creating_candidates() -> Result<()> {
        let fixture = Fixture::new()?;
        let root = fixture.work().join("decoded_images");
        fs::create_dir_all(root.join("vendor"))?;
        fs::write(
            root.join("vendor/fixture.blob"),
            b"protected synthetic data",
        )?;
        fs::write(root.join("visible.blob"), b"visible")?;
        let plan = status_for(&fixture.runtime)?;
        assert!(plan.errors.is_empty(), "{:?}", plan.errors);
        assert!(plan.disk_usage.partial);
        assert_eq!((plan.disk_usage.count, plan.disk_usage.bytes), (1, 7));
        assert_eq!(plan.disk_usage.skipped_count, 1);
        assert!(plan
            .disk_usage
            .errors
            .iter()
            .any(|issue| same(&issue.path, &root.join("vendor"))));
        assert!(plan.files.is_empty());
        assert!(root.join("vendor/fixture.blob").exists());
        Ok(())
    }

    #[test]
    fn key_plan_requires_exact_account_and_is_separate_from_normal_plans() -> Result<()> {
        let fixture = Fixture::new()?;
        let inputs = PlanInputs::default();
        let ordinary = plan_for(&fixture.runtime, &inputs, None)?;
        assert!(!ordinary.key_removal_authorized);
        assert!(ordinary
            .files
            .iter()
            .all(|file| file.category != Category::KeyCache));
        assert!(
            plan_with_key_removal_for(&fixture.runtime, &inputs, None, Some("wrong-account"))
                .is_err()
        );
        assert!(plan_with_key_removal_for(
            &fixture.runtime,
            &inputs,
            None,
            Some(&fixture.runtime.id[..8])
        )
        .is_err());
        assert!(plan_with_key_removal_for(
            &fixture.runtime,
            &inputs,
            Some(&fixture.runtime.id),
            Some(&fixture.runtime.id)
        )
        .is_err());
        let mixed = fixture.inventory("chat.txt", b"synthetic chat")?;
        assert!(plan_with_key_removal_for(
            &fixture.runtime,
            &mixed,
            None,
            Some(&fixture.runtime.id)
        )
        .is_err());

        let plan = fixture.key_plan()?;
        assert!(plan.errors.is_empty(), "{:?}", plan.errors);
        assert!(plan.key_removal_authorized);
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].category, Category::KeyCache);
        assert!(same(&plan.files[0].path, &fixture.runtime.config.keys_file));
        assert_eq!(plan.bytes, FAKE_KEY.len() as u64);
        assert!(!serde_json::to_string(&plan)?.contains("SYNTHETIC-NOT-A-WECHAT-KEY"));
        assert!(!fixture.runtime.directory.exists());
        Ok(())
    }

    #[test]
    fn key_execution_requires_reauthorization_and_exact_selection() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.ready_runtime()?;
        let plan = fixture.key_plan()?;
        assert!(plan.errors.is_empty(), "{:?}", plan.errors);
        let mut options = fixture.options(&plan);
        assert!(execute_for(&fixture.runtime, &plan, &options).is_err());
        assert!(execute_with_key_removal_for(
            &fixture.runtime,
            &plan,
            &options,
            Some("wrong-account")
        )
        .is_err());
        let ordinary = status_for(&fixture.runtime)?;
        assert!(execute_with_key_removal_for(
            &fixture.runtime,
            &ordinary,
            &options,
            Some(&fixture.runtime.id)
        )
        .is_err());
        options.confirm_account = "wrong-account".into();
        assert!(execute_with_key_removal_for(
            &fixture.runtime,
            &plan,
            &options,
            Some(&fixture.runtime.id)
        )
        .is_err());
        options.confirm_account = fixture.runtime.id.clone();
        for selection in [
            vec!["all".into()],
            vec![plan.files[0].id[..8].into()],
            vec![hash(b"not-in-the-plan")],
            vec![plan.files[0].id.clone(); 2],
        ] {
            options.select = selection;
            assert!(execute_with_key_removal_for(
                &fixture.runtime,
                &plan,
                &options,
                Some(&fixture.runtime.id)
            )
            .is_err());
            assert_eq!(fs::read(&fixture.runtime.config.keys_file)?, FAKE_KEY);
        }
        Ok(())
    }

    #[test]
    fn authorized_key_execution_deletes_only_current_fixture_and_respects_locks() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.ready_runtime()?;
        let other_key = fixture.work().join("all_keys_other.json");
        let dpapi = fixture.work().join("account_key.dpapi");
        fs::write(&other_key, b"synthetic other key")?;
        fs::write(&dpapi, b"synthetic DPAPI placeholder")?;
        let plan = fixture.key_plan()?;
        assert!(plan.errors.is_empty(), "{:?}", plan.errors);
        let options = fixture.options(&plan);
        let locks = handles::runtime_locks(&fixture.runtime.directory)?;
        assert!(execute_with_key_removal_for(
            &fixture.runtime,
            &plan,
            &options,
            Some(&fixture.runtime.id)
        )
        .is_err());
        assert_eq!(fs::read(&fixture.runtime.config.keys_file)?, FAKE_KEY);
        drop(locks);
        let report = execute_with_key_removal_for(
            &fixture.runtime,
            &plan,
            &options,
            Some(&fixture.runtime.id),
        )?;
        assert!(report.complete, "{:?}", report.errors);
        assert_eq!((report.count, report.bytes), (1, FAKE_KEY.len() as u64));
        assert_eq!(report.directories_removed, 0);
        assert!(!fixture.runtime.config.keys_file.exists());
        assert_eq!(fs::read(&other_key)?, b"synthetic other key");
        assert_eq!(fs::read(&dpapi)?, b"synthetic DPAPI placeholder");
        assert!(fixture.runtime.config_path.exists());
        assert_eq!(
            fs::read(fixture.runtime.config.db_dir.join("fixture.db"))?,
            b"synthetic source, not a database"
        );
        Ok(())
    }

    #[test]
    fn changed_key_is_rejected_before_any_deletion() -> Result<()> {
        let fixture = Fixture::new()?;
        fixture.ready_runtime()?;
        let plan = fixture.key_plan()?;
        assert!(plan.errors.is_empty(), "{:?}", plan.errors);
        fs::write(&fixture.runtime.config.keys_file, b"synthetic changed key")?;
        assert!(execute_with_key_removal_for(
            &fixture.runtime,
            &plan,
            &fixture.options(&plan),
            Some(&fixture.runtime.id)
        )
        .is_err());
        assert_eq!(
            fs::read(&fixture.runtime.config.keys_file)?,
            b"synthetic changed key"
        );
        Ok(())
    }

    #[test]
    fn configured_dpapi_file_is_protected_even_with_key_authorization() -> Result<()> {
        let fixture = Fixture::with_key_name("account_key.dpapi")?;
        let plan = fixture.key_plan()?;
        assert!(plan.key_removal_authorized);
        assert!(!plan.errors.is_empty());
        assert!(plan.files.is_empty());
        assert_eq!(plan.count, 0);
        assert_eq!(fs::read(&fixture.runtime.config.keys_file)?, FAKE_KEY);
        assert!(!fixture.runtime.directory.exists());
        Ok(())
    }
}
