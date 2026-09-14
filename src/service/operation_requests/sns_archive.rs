use std::path::PathBuf;

#[derive(Debug, Default, serde::Serialize, serde::Deserialize, Clone)]
pub struct Args {
    /// 输出基础目录；其下创建朋友圈图片，默认选中配置旁 wechat_files/<账号>
    pub output_dir: Option<PathBuf>,
    /// 显式认领未绑定来源的旧归档目录，已有来源冲突仍拒绝
    pub adopt_existing: bool,
}
