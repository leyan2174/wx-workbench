//! Framing only. Callers wrap the entire exchange in one deadline/cancellation scope.
use std::{fmt, io};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt,
};
use zeroize::Zeroizing;

pub(crate) const HARD_LIMIT: usize = 256 * 1024 * 1024;

#[derive(Debug)]
pub(crate) enum FrameError {
    Eof,
    Incomplete,
    Oversize,
    InvalidLimit,
    Protocol,
    Timeout,
    Io(io::Error),
}
impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Eof => "peer closed before frame",
            Self::Incomplete => "incomplete frame",
            Self::Oversize => "frame exceeds size limit",
            Self::InvalidLimit => "invalid frame budget",
            Self::Protocol => "unsupported or mismatched query protocol/runtime",
            Self::Timeout => "communication deadline exceeded",
            Self::Io(_) => "frame IO failed",
        })
    }
}
impl std::error::Error for FrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}
impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
pub(crate) fn budget(limit: usize) -> Result<(), FrameError> {
    if limit == 0 || limit > HARD_LIMIT {
        Err(FrameError::InvalidLimit)
    } else {
        Ok(())
    }
}
pub(crate) async fn line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> Result<Zeroizing<Vec<u8>>, FrameError> {
    budget(limit)?;
    let mut bytes = Zeroizing::new(Vec::new());
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Err(if bytes.is_empty() {
                FrameError::Eof
            } else {
                FrameError::Incomplete
            });
        }
        let newline = available.iter().position(|b| *b == b'\n');
        let count = newline.map_or(available.len(), |i| i + 1);
        if count > limit - bytes.len() {
            return Err(FrameError::Oversize);
        }
        bytes.extend_from_slice(&available[..count]);
        reader.consume(count);
        if newline.is_some() {
            return Ok(bytes);
        }
    }
}
pub(crate) async fn length<R: AsyncRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> Result<Zeroizing<Vec<u8>>, FrameError> {
    budget(limit)?;
    let mut header = [0; 4];
    exact(reader, &mut header, true).await?;
    let size = u32::from_le_bytes(header) as usize;
    if size == 0 {
        return Err(FrameError::Protocol);
    }
    if size > limit {
        return Err(FrameError::Oversize);
    }
    let mut bytes = Zeroizing::new(vec![0; size]);
    exact(reader, &mut bytes, false).await?;
    Ok(bytes)
}
async fn exact<R: AsyncRead + Unpin>(
    reader: &mut R,
    bytes: &mut [u8],
    header: bool,
) -> Result<(), FrameError> {
    let mut offset = 0;
    while offset < bytes.len() {
        let n = reader.read(&mut bytes[offset..]).await?;
        if n == 0 {
            return Err(if header && offset == 0 {
                FrameError::Eof
            } else {
                FrameError::Incomplete
            });
        }
        offset += n;
    }
    Ok(())
}
pub(crate) async fn write_line<W: AsyncWrite + Unpin>(
    writer: &mut W,
    bytes: &[u8],
    limit: usize,
) -> Result<(), FrameError> {
    budget(limit)?;
    if bytes.len() >= limit {
        return Err(FrameError::Oversize);
    }
    writer.write_all(bytes).await?;
    writer.write_all(b"\n").await?;
    Ok(())
}
pub(crate) async fn write_length<W: AsyncWrite + Unpin>(
    writer: &mut W,
    bytes: &[u8],
) -> Result<(), FrameError> {
    budget(bytes.len())?;
    writer.write_u32_le(bytes.len() as u32).await?;
    writer.write_all(bytes).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

    #[tokio::test]
    async fn real_length_pipe_limits_truncation_deadline_and_cancellation() {
        for mode in [
            "ok", "oversize", "header", "body", "eof", "timeout", "cancel",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let name = format!(
                r"\\.\pipe\wx-frame-{}-{}-{mode}",
                std::process::id(),
                directory.path().file_name().unwrap().to_string_lossy()
            );
            let mut server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&name)
                .unwrap();
            let mut client = ClientOptions::new().open(&name).unwrap();
            server.connect().await.unwrap();
            let sending = tokio::spawn(async move {
                match mode {
                    "ok" => write_length(&mut server, b"{}").await.unwrap(),
                    "oversize" => server.write_u32_le(1025).await.unwrap(),
                    "header" => server.write_all(&[2, 0]).await.unwrap(),
                    "body" => {
                        server.write_u32_le(4).await.unwrap();
                        server.write_all(b"{}").await.unwrap();
                    }
                    _ => (),
                }
                if matches!(mode, "header" | "body" | "eof") {
                    tokio::time::sleep(Duration::from_millis(30)).await;
                } else {
                    let mut ack = [0];
                    let _ = server.read(&mut ack).await;
                }
            });
            if mode == "cancel" {
                let read = length(&mut client, 1024);
                tokio::pin!(read);
                tokio::select! {
                    result = &mut read => panic!("unexpected completion: {result:?}"),
                    _ = tokio::time::sleep(Duration::from_millis(20)) => (),
                }
            } else {
                let result =
                    tokio::time::timeout(Duration::from_millis(100), length(&mut client, 1024))
                        .await;
                match mode {
                    "ok" => assert_eq!(&**result.unwrap().unwrap(), b"{}"),
                    "oversize" => assert!(matches!(result.unwrap(), Err(FrameError::Oversize))),
                    "header" | "body" => {
                        assert!(matches!(result.unwrap(), Err(FrameError::Incomplete)))
                    }
                    "eof" => assert!(matches!(result.unwrap(), Err(FrameError::Eof))),
                    "timeout" => assert!(result.is_err()),
                    _ => unreachable!(),
                }
            }
            drop(client);
            tokio::time::timeout(Duration::from_secs(1), sending)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[test]
    fn budgets_are_finite_and_keep_large_exports_available() {
        assert!(budget(0).is_err());
        assert!(budget(usize::MAX).is_err());
        assert!(budget(HARD_LIMIT + 1).is_err());
        assert!(budget(256 * 1024 * 1024).is_ok());
    }
}
