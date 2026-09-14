//! Synthetic daemon tests only; callers own and launch the known fixture process.
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

pub async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    stream: S,
    pipe: &str,
    request: Value,
) -> Result<Value, String> {
    let runtime = pipe
        .strip_prefix("wx-cli-v2-")
        .ok_or("invalid fixture runtime")?;
    let mut reader = BufReader::new(stream);
    let mut hello = String::new();
    (&mut reader)
        .take(1025)
        .read_line(&mut hello)
        .await
        .map_err(|e| e.to_string())?;
    if hello.len() > 1024 || !hello.ends_with('\n') {
        return Err("invalid query hello frame".into());
    }
    let hello: Value = serde_json::from_str(&hello).map_err(|e| e.to_string())?;
    if hello["version"] != 3 || hello["runtime_id"].as_str() != Some(runtime) {
        return Err("query hello identity mismatch".into());
    }
    let envelope =
        json!({"version":3,"runtime_id":runtime,"response_limit":1024*1024,"request":request});
    reader
        .get_mut()
        .write_all(format!("{envelope}\n").as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let mut reply = String::new();
    (&mut reader)
        .take(1024 * 1024 + 1)
        .read_line(&mut reply)
        .await
        .map_err(|e| e.to_string())?;
    if reply.len() > 1024 * 1024 || !reply.ends_with('\n') {
        return Err("invalid query reply frame".into());
    }
    let reply: Value = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
    if reply["version"] != 3
        || reply["runtime_id"].as_str() != Some(runtime)
        || reply["result"] != "response"
    {
        return Err("query reply identity or budget mismatch".into());
    }
    Ok(reply["response"].clone())
}
