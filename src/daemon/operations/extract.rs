use super::{
    output::{print_value, resolve},
    transport,
};
use crate::ipc::Request;
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
