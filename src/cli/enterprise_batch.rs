//! 企业微信完整批量 CLI；main 只需注册 Args 并调用 cmd，不在模块加载时做任何扫描。
use crate::toolkit::enterprise::queries::ExportFormat;
use crate::toolkit::enterprise_batch::{
    self as batch, ExportOptions, KeyOptions, ProcessScanner, ScanOptions,
};
use anyhow::{ensure, Result};
use std::path::PathBuf;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(clap::Subcommand)]
pub enum Command {
    /// 列出企业微信账号 Data 目录，不自动选择账号
    Discover {
        #[arg(long)]
        root: Option<PathBuf>,
    },
    /// 显式授权扫描；只打印匹配状态，可另存私有账号绑定密钥文件
    Scan(ScanArgs),
    /// 批量解密单账号离线主库；拒绝 WAL/SHM/journal
    Decrypt(DecryptArgs),
    /// 列出离线快照中的全部有消息会话
    List {
        #[arg(long)]
        snapshot: PathBuf,
        #[arg(long)]
        self_id: Option<i64>,
    },
    /// 批量导出全部或指定会话，支持 CSV/HTML/JSON 组合
    Export(ExportArgs),
    /// 授权取钥、批量主库解密及多会话目录导出
    Run(RunArgs),
}

#[derive(clap::Args)]
pub struct MemoryArgs {
    /// 明确授权读取本机 WXWork.exe 进程内存
    #[arg(long)]
    pub authorize_memory_scan: bool,
    /// 仅扫描指定企业微信 PID，可重复或逗号分隔
    #[arg(long, value_delimiter = ',')]
    pub pid: Vec<u32>,
    #[arg(long)]
    pub scan_bare_hex: bool,
    #[arg(long)]
    pub no_cipher_structs: bool,
    #[arg(long, default_value_t = 120)]
    pub scan_timeout: u64,
    #[arg(long, default_value_t = 4096)]
    pub scan_max_mib: u64,
}

impl MemoryArgs {
    pub fn options(&self) -> Result<ScanOptions> {
        let options = ScanOptions {
            authorized: self.authorize_memory_scan,
            pids: self.pid.clone(),
            bare_hex: self.scan_bare_hex,
            cipher_structs: !self.no_cipher_structs,
            max_seconds: self.scan_timeout,
            max_bytes: self
                .scan_max_mib
                .checked_mul(1024 * 1024)
                .ok_or_else(|| anyhow::anyhow!("扫描字节预算溢出"))?,
        };
        options.validate()?;
        Ok(options)
    }
}

#[derive(clap::Args)]
pub struct ScanArgs {
    #[arg(long)]
    pub data_dir: PathBuf,
    /// 完整扫描成功后保存账号绑定 JSON；绝对路径，父目录须存在且与账号隔离。
    /// 不覆盖已有文件；重复扫描请明确选择新文件名，再交给 --keys-file 使用。
    #[arg(long)]
    pub keys_output: Option<PathBuf>,
    #[command(flatten)]
    pub memory: MemoryArgs,
}

#[derive(clap::Args)]
pub struct KeyArgs {
    /// 32 位 hex 全局兜底密钥文件；不接受命令行明文密钥
    #[arg(long)]
    pub key_file: Option<PathBuf>,
    /// 带 _db_dir 账号绑定的逐库密钥 JSON；优先于全局密钥，错误条目直接失败
    #[arg(long)]
    pub keys_file: Option<PathBuf>,
    /// 文件密钥不足时，从已授权企业微信进程取钥
    #[arg(long, requires = "authorize_memory_scan")]
    pub auto_keys: bool,
    #[command(flatten)]
    pub memory: MemoryArgs,
}

impl KeyArgs {
    pub fn options(&self) -> Result<KeyOptions> {
        ensure!(
            self.auto_keys || !self.memory.authorize_memory_scan,
            "--authorize-memory-scan 需与 --auto-keys 一起使用"
        );
        Ok(KeyOptions {
            key_file: self.key_file.clone(),
            keys_file: self.keys_file.clone(),
            auto_scan: if self.auto_keys {
                Some(self.memory.options()?)
            } else {
                None
            },
        })
    }
}

#[derive(clap::Args)]
pub struct DecryptArgs {
    #[arg(long)]
    pub data_dir: PathBuf,
    /// 尚不存在的新目录；父目录必须已存在
    #[arg(long)]
    pub output: PathBuf,
    #[command(flatten)]
    pub keys: KeyArgs,
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Format {
    Csv,
    Html,
    Json,
}

#[derive(clap::Args)]
pub struct SelectionArgs {
    /// 不指定则导出全部有消息的会话
    #[arg(long = "conversation", value_delimiter = ',')]
    pub conversations: Vec<String>,
    #[arg(long, value_enum, value_delimiter = ',', default_value = "csv")]
    pub formats: Vec<Format>,
    /// 明确本人 ID；不从路径推断身份
    #[arg(long)]
    pub self_id: Option<i64>,
}
impl SelectionArgs {
    pub fn options(&self) -> ExportOptions {
        ExportOptions {
            conversations: self.conversations.clone(),
            self_id: self.self_id,
            formats: self
                .formats
                .iter()
                .map(|format| match format {
                    Format::Csv => ExportFormat::Csv,
                    Format::Html => ExportFormat::Html,
                    Format::Json => ExportFormat::Json,
                })
                .collect(),
        }
    }
}

#[derive(clap::Args)]
pub struct ExportArgs {
    #[arg(long)]
    pub snapshot: PathBuf,
    #[arg(long)]
    pub output: PathBuf,
    #[command(flatten)]
    pub selection: SelectionArgs,
}

#[derive(clap::Args)]
pub struct RunArgs {
    #[arg(long)]
    pub data_dir: PathBuf,
    /// 解密批次新目录，成功后的快照位置由报告返回
    #[arg(long)]
    pub decrypted_output: PathBuf,
    /// 会话导出批次新目录，不得位于解密快照内
    #[arg(long)]
    pub export_output: PathBuf,
    #[command(flatten)]
    pub keys: KeyArgs,
    #[command(flatten)]
    pub selection: SelectionArgs,
}

pub fn cmd(args: Args) -> Result<()> {
    #[cfg(windows)]
    {
        cmd_with_scanner(args, Some(&batch::windows::WindowsScanner))
    }
    #[cfg(not(windows))]
    {
        cmd_with_scanner(args, None)
    }
}

/// main 可注入 scanner 通用只读进程接口；None 仍支持全部离线解密/导出。
pub fn cmd_with_scanner(args: Args, scanner: Option<&dyn ProcessScanner>) -> Result<()> {
    match args.command {
        Command::Discover { root } => print_report(&batch::discover_accounts(root.as_deref())?),
        Command::List { snapshot, self_id } => {
            print_report(&batch::list_conversations(&snapshot, self_id)?)
        }
        Command::Scan(args) => {
            let options = args.memory.options()?;
            let scanner =
                scanner.ok_or_else(|| anyhow::anyhow!("当前平台没有企业微信进程扫描适配器"))?;
            let output = args
                .keys_output
                .as_deref()
                .map(|path| batch::KeyRing::prepare_output(&args.data_dir, path))
                .transpose()?;
            let (keys, report) = batch::scan::scan_authorized(&args.data_dir, &options, scanner)?;
            print_report(&report)?;
            ensure!(report.failed == 0, "企业微信扫描未完整完成，详见无密钥报告");
            if let Some(output) = output {
                output.publish(&keys)?;
            }
            Ok(())
        }
        Command::Decrypt(args) => {
            let report =
                batch::decrypt_batch(&args.data_dir, &args.output, &args.keys.options()?, scanner)?;
            print_report(&report)?;
            ensure!(report.failed == 0, "部分企业微信数据库未解密，详见批次报告");
            Ok(())
        }
        Command::Export(args) => {
            let report =
                batch::export_batch(&args.snapshot, &args.output, &args.selection.options())?;
            print_report(&report)?;
            ensure!(
                report.failed == 0,
                "部分企业微信会话或格式导出失败，详见批次报告"
            );
            Ok(())
        }
        Command::Run(args) => {
            args.selection.options().validate()?;
            batch::validate_run_outputs(
                &args.data_dir,
                &args.decrypted_output,
                &args.export_output,
            )?;
            let decrypt = batch::decrypt_batch(
                &args.data_dir,
                &args.decrypted_output,
                &args.keys.options()?,
                scanner,
            )?;
            // 不在缺失库或旁路日志失败后生成看似完整的导出；独立 Export 可显式处理已知部分快照。
            if decrypt.failed != 0 {
                print_report(
                    &serde_json::json!({"engine": "rust", "decrypt": decrypt, "export": null, "export_skipped": true}),
                )?;
                anyhow::bail!("解密不完整，未执行导出；成功主库已保留，详见解密报告");
            }
            match batch::export_batch(
                &decrypt.output,
                &args.export_output,
                &args.selection.options(),
            ) {
                Ok(export) => {
                    let failed = export.failed;
                    print_report(
                        &serde_json::json!({"engine": "rust", "decrypt": decrypt, "export": export}),
                    )?;
                    ensure!(failed == 0, "部分企业微信会话或格式导出失败，详见导出报告");
                    Ok(())
                }
                Err(error) => {
                    print_report(
                        &serde_json::json!({"engine": "rust", "decrypt": decrypt, "export": null, "export_failed": true}),
                    )?;
                    Err(error)
                }
            }
        }
    }
}

fn print_report(report: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(report)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: Args,
    }

    #[test]
    fn keys_output_belongs_only_to_scan_and_key_inputs_can_combine() {
        assert!(Cli::try_parse_from([
            "test",
            "scan",
            "--data-dir",
            "account",
            "--authorize-memory-scan",
            "--keys-output",
            "keys.json"
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "test",
            "decrypt",
            "--data-dir",
            "account",
            "--output",
            "out",
            "--key-file",
            "global.txt",
            "--keys-file",
            "per-db.json"
        ])
        .is_ok());
        for command in ["decrypt", "run", "export", "discover", "list"] {
            let error = Cli::try_parse_from(["test", command, "--keys-output", "keys.json"])
                .err()
                .unwrap();
            assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
        }
    }

    #[test]
    fn unavailable_or_failed_scan_never_creates_keys_output() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("account");
        let outputs = root.path().join("keys");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&outputs).unwrap();
        std::fs::write(
            source.join("a.db"),
            include_bytes!("../../tests/fixtures/enterprise/header.enc"),
        )
        .unwrap();
        let target = outputs.join("saved.json");
        let make = || Args {
            command: Command::Scan(ScanArgs {
                data_dir: source.clone(),
                keys_output: Some(target.clone()),
                memory: MemoryArgs {
                    authorize_memory_scan: true,
                    pid: vec![],
                    scan_bare_hex: false,
                    no_cipher_structs: false,
                    scan_timeout: 1,
                    scan_max_mib: 1,
                },
            }),
        };
        assert!(cmd_with_scanner(make(), None).is_err());
        assert!(!target.exists());
        assert!(cmd_with_scanner(make(), Some(&NoProcess)).is_err());
        assert!(!target.exists());
        assert_eq!(std::fs::read_dir(outputs).unwrap().count(), 0);
    }

    struct NoProcess;
    impl ProcessScanner for NoProcess {
        fn wxwork_pids(&self) -> Result<Vec<u32>> {
            Ok(vec![])
        }
        fn open(&self, _: u32) -> Result<Box<dyn batch::ProcessMemory>> {
            panic!("合成测试不得读取进程")
        }
    }

    #[test]
    fn successful_synthetic_scan_publishes_readable_keys_file() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("account");
        let outputs = root.path().join("keys");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&outputs).unwrap();
        std::fs::write(
            source.join("a.db"),
            include_bytes!("../../tests/fixtures/enterprise/header.enc"),
        )
        .unwrap();
        let target = outputs.join("saved.json");
        let args = Args {
            command: Command::Scan(ScanArgs {
                data_dir: source.clone(),
                keys_output: Some(target.clone()),
                memory: MemoryArgs {
                    authorize_memory_scan: true,
                    pid: vec![7],
                    scan_bare_hex: false,
                    no_cipher_structs: true,
                    scan_timeout: 5,
                    scan_max_mib: 1,
                },
            }),
        };
        cmd_with_scanner(args, Some(&SyntheticScanner)).unwrap();
        let report = batch::decrypt_batch(
            &source,
            &root.path().join("decrypted"),
            &KeyOptions {
                keys_file: Some(target),
                ..Default::default()
            },
            None,
        )
        .unwrap();
        assert_eq!((report.decrypted, report.failed), (1, 0));
    }

    struct SyntheticScanner;
    impl ProcessScanner for SyntheticScanner {
        fn wxwork_pids(&self) -> Result<Vec<u32>> {
            Ok(vec![7])
        }
        fn open(&self, pid: u32) -> Result<Box<dyn batch::ProcessMemory>> {
            assert_eq!(pid, 7);
            Ok(Box::new(SyntheticMemory))
        }
    }
    struct SyntheticMemory;
    impl batch::ProcessMemory for SyntheticMemory {
        fn pointer_width(&self) -> Result<u8> {
            Ok(8)
        }
        fn regions(&self) -> Result<Vec<batch::scan::MemoryRegion>> {
            Ok(vec![batch::scan::MemoryRegion {
                base: 0x1000,
                size: 35,
            }])
        }
        fn read(&self, address: u64, buffer: &mut [u8]) -> Result<usize> {
            let bytes = b"x'00112233445566778899aabbccddeeff'";
            let start = usize::try_from(address.checked_sub(0x1000).unwrap()).unwrap();
            buffer.copy_from_slice(&bytes[start..start + buffer.len()]);
            Ok(buffer.len())
        }
    }
}
