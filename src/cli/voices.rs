//! 只适配参数并转发后台操作，不在 CLI 中读取语音数据。
pub use crate::daemon::operations::voices::Args;
use crate::service::operations::Operation;
use anyhow::Result;

pub fn cmd_voices(args: Args) -> Result<()> {
    let Args {
        chat,
        output,
        limit,
        offset,
        since,
        until,
        overwrite,
        json: json_output,
    } = args;
    crate::service::operation_client::run(Operation::Voices {
        chat,
        output,
        limit,
        offset,
        since,
        until,
        overwrite,
        json_output,
    })
}
