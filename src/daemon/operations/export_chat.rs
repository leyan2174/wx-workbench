//! 原生单聊导出的文件发布边界，与原有分页 export 命令独立。
use anyhow::Result;
use std::path::PathBuf;

pub fn cmd_export(chat: String, output: PathBuf) -> Result<()> {
    let runtime = crate::runtime::RuntimeContext::load()?;
    let output = std::path::absolute(output)?;
    validate_output_for(&runtime, &output)?;
    let target = crate::infrastructure::publication::ExportTarget::capture(&runtime, &output)?;
    let response =
        crate::service::query_client::send_for(&runtime, crate::ipc::Request::ExportChat { chat })?;
    target.write_json(&response.data)?;
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
    crate::infrastructure::publication::validate_export_target(runtime, output)
}

/// 固定账号的校验与原子发布，不重新读取配置。
#[cfg(test)]
pub(super) fn write_document_for(
    runtime: &crate::runtime::RuntimeContext,
    output: &std::path::Path,
    data: &serde_json::Value,
) -> Result<()> {
    crate::infrastructure::publication::ExportTarget::capture(runtime, output)?.write_json(data)
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
                key_store: None,
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
