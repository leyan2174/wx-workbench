//! 原生单聊导出的文件发布边界，与原有分页 export 命令独立。
use anyhow::Result;
use std::path::PathBuf;

pub fn cmd_export(chat: String, output: PathBuf) -> Result<()> {
    let runtime = crate::runtime::RuntimeContext::load()?;
    let output = std::path::absolute(output)?;
    validate_output_for(&runtime, &output)?;
    let response = super::transport::send_for(&runtime, crate::ipc::Request::ExportChat { chat })?;
    write_document_for(&runtime, &output, &response.data)?;
    println!(
        "已导出 {} 条消息至 {}",
        response.data["messages"].as_array().map_or(0, Vec::len),
        output.display()
    );
    Ok(())
}

pub(super) fn validate_output_for(
    runtime: &crate::runtime::RuntimeContext,
    output: &std::path::Path,
) -> Result<()> {
    let config = &runtime.config;
    for source in [&config.db_dir, &config.decrypted_dir, &runtime.directory] {
        crate::toolkit::separate(source, output)?;
    }
    if output.exists() {
        let destination = output.canonicalize()?;
        for protected in [&config.keys_file, &runtime.config_path] {
            if protected.exists() {
                anyhow::ensure!(
                    !destination
                        .to_string_lossy()
                        .eq_ignore_ascii_case(&protected.canonicalize()?.to_string_lossy()),
                    "导出不能覆盖账号配置或密钥文件"
                );
            }
        }
    }
    Ok(())
}

/// 固定账号的校验与原子发布，不重新读取配置。
pub(super) fn write_document_for(
    runtime: &crate::runtime::RuntimeContext,
    output: &std::path::Path,
    data: &serde_json::Value,
) -> Result<()> {
    validate_output_for(runtime, output)?;
    crate::toolkit::atomic_output(output, |temporary| {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(temporary)?;
        serde_json::to_writer_pretty(file, data)?;
        Ok(())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_runtime_writer_does_not_reload_changed_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let runtime = crate::runtime::RuntimeContext {
            config: crate::config::Config {
                db_dir: root.join("db"),
                keys_file: root.join("keys.json"),
                decrypted_dir: root.join("decrypted"),
                wechat_process: String::new(),
            },
            config_path: root.join("config.json"),
            root: root.to_owned(),
            id: "synthetic".into(),
            directory: root.join("runtime"),
        };
        // 固定上下文后磁盘配置发生变化，写出不应重新解析它。
        std::fs::write(&runtime.config_path, b"changed configuration").unwrap();
        std::fs::write(&runtime.config.keys_file, b"protected synthetic keys").unwrap();
        let output = root.join("export.json");
        let doc = serde_json::json!({"username":"synthetic","messages":[],"custom":true});
        write_document_for(&runtime, &output, &doc).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(output).unwrap()).unwrap(),
            doc
        );
        for protected in [
            runtime.config.db_dir.join("export.json"),
            runtime.config.decrypted_dir.join("export.json"),
            runtime.directory.join("export.json"),
            runtime.config_path.clone(),
            runtime.config.keys_file.clone(),
        ] {
            assert!(write_document_for(&runtime, &protected, &doc).is_err());
        }
        assert_eq!(
            std::fs::read(&runtime.config_path).unwrap(),
            b"changed configuration"
        );
        assert_eq!(
            std::fs::read(&runtime.config.keys_file).unwrap(),
            b"protected synthetic keys"
        );
    }
}
