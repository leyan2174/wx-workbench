use super::output::{print_value, resolve};
use crate::ipc::Request;
use crate::service::query_client as transport;
use anyhow::Result;

pub(super) fn execute(
    attachment_id: String,
    output: String,
    overwrite: bool,
    json: bool,
) -> Result<()> {
    let output = std::path::absolute(output)?.to_string_lossy().into_owned();
    let response = transport::send(Request::Extract {
        attachment_id,
        output,
        overwrite,
    })?;
    print_value(&response.data, &resolve(json))
}
