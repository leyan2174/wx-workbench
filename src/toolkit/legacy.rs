//! 保留本地语音兼容与诊断所需的路径发现，不再调度旧业务脚本。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const BUNDLED_WECHAT_DECRYPT_DIR: &str = r"vendor\wechat-decrypt";

pub(crate) fn toolkit_root() -> PathBuf {
    if let Some(root) = std::env::var_os("WX_WECHAT_DECRYPT_DIR") {
        return PathBuf::from(root);
    }

    for candidate in toolkit_root_candidates() {
        if candidate.is_dir() {
            return candidate;
        }
    }

    PathBuf::from(BUNDLED_WECHAT_DECRYPT_DIR)
}

fn toolkit_root_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join(BUNDLED_WECHAT_DECRYPT_DIR));
            if let Some(project_dir) = exe_dir.parent().and_then(|target_dir| target_dir.parent()) {
                candidates.push(project_dir.join(BUNDLED_WECHAT_DECRYPT_DIR));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(BUNDLED_WECHAT_DECRYPT_DIR));
    }
    candidates.push(PathBuf::from(BUNDLED_WECHAT_DECRYPT_DIR));
    candidates
}

pub(crate) fn toolkit_python() -> PathBuf {
    if let Some(python) = std::env::var_os("WX_WECHAT_DECRYPT_PYTHON") {
        return PathBuf::from(python);
    }

    for candidate in toolkit_python_candidates() {
        if candidate.is_file() {
            return candidate;
        }
    }

    PathBuf::from("python")
}

fn toolkit_python_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join(".venv").join("Scripts").join("python.exe"));
            candidates.push(exe_dir.join("python").join("python.exe"));
            if let Some(project_dir) = exe_dir.parent().and_then(|target_dir| target_dir.parent()) {
                candidates.push(project_dir.join(".venv").join("Scripts").join("python.exe"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(".venv").join("Scripts").join("python.exe"));
    }
    candidates
}

pub(crate) fn python_available(python: &Path) -> bool {
    if python.is_file() {
        return true;
    }
    if python.components().count() == 1 {
        return Command::new(python)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
    }
    false
}
