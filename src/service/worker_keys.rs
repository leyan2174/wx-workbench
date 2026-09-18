//! Private stdin capability contract. Never put Access in argv, environment, or logs.
use std::{
    collections::HashMap,
    fmt,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

use anyhow::{anyhow, ensure, Result};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use super::{client, protocol::Call};
use crate::runtime::RuntimeContext;

#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
        #[cfg(test)]
        observe_drop(self.0.is_empty());
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "material",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum MaterialChange {
    Account(Vec<u8>),
    Databases(HashMap<String, String>),
    Image { aes: [u8; 16], xor: u8 },
}

impl fmt::Debug for MaterialChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Account(_) => "Account([redacted])",
            Self::Databases(_) => "Databases([redacted])",
            Self::Image { .. } => "Image([redacted])",
        })
    }
}

impl Drop for MaterialChange {
    fn drop(&mut self) {
        match self {
            Self::Account(bytes) => {
                bytes.zeroize();
                #[cfg(test)]
                observe_drop(bytes.is_empty());
            }
            Self::Databases(keys) => {
                for (mut name, mut key) in keys.drain() {
                    name.zeroize();
                    key.zeroize();
                    #[cfg(test)]
                    observe_drop(name.is_empty() && key.is_empty());
                }
            }
            Self::Image { aes, xor } => {
                aes.zeroize();
                xor.zeroize();
                #[cfg(test)]
                observe_drop(*aes == [0; 16] && *xor == 0);
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateRequest {
    pub capability: Secret,
    pub expected_revision: u64,
    pub changes: Vec<MaterialChange>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevisionRequest {
    pub capability: Secret,
    pub expected_revision: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseReadRequest {
    pub capability: Secret,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageReadRequest {
    pub capability: Secret,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageMaterial {
    pub aes: [u8; 16],
    pub xor: u8,
}

impl std::fmt::Debug for ImageMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ImageMaterial([REDACTED])")
    }
}

impl Drop for ImageMaterial {
    fn drop(&mut self) {
        self.aes.zeroize();
        self.xor.zeroize();
        #[cfg(test)]
        observe_drop(self.aes == [0; 16] && self.xor == 0);
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountMaterial(Vec<u8>);

impl AccountMaterial {
    pub fn new(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() == 32, "invalid account key material");
        Ok(Self(bytes.to_vec()))
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for AccountMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccountMaterial([REDACTED])")
    }
}

impl Drop for AccountMaterial {
    fn drop(&mut self) {
        self.0.zeroize();
        #[cfg(test)]
        observe_drop(self.0.is_empty());
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitSeed {
    pub has_database_keys: bool,
    pub account: Option<AccountMaterial>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DatabaseKeys(pub HashMap<String, String>);

impl std::fmt::Debug for DatabaseKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DatabaseKeys([REDACTED])")
    }
}

impl Drop for DatabaseKeys {
    fn drop(&mut self) {
        for (mut name, mut key) in self.0.drain() {
            name.zeroize();
            key.zeroize();
            #[cfg(test)]
            observe_drop(name.is_empty() && key.is_empty());
        }
    }
}

pub struct DatabaseSnapshot {
    pub revision: u64,
    pub databases: Option<DatabaseKeys>,
}

impl fmt::Debug for DatabaseSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatabaseSnapshot")
            .field("revision", &self.revision)
            .field("databases", &self.databases.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

pub struct ImageSnapshot {
    pub revision: u64,
    pub material: Option<ImageMaterial>,
}

impl fmt::Debug for ImageSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageSnapshot")
            .field("revision", &self.revision)
            .field("material", &self.material.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

const DATABASE_REPLY_MAGIC: &[u8; 8] = b"WXDBKEY1";
pub(crate) const MAX_DATABASE_REPLY_BYTES: usize = 8 * 1024 * 1024;
const IMAGE_REPLY_MAGIC: &[u8; 8] = b"WXIMKEY1";
pub(crate) const MAX_IMAGE_REPLY_BYTES: usize = 64 * 1024;
const MAX_DATABASE_KEYS: usize = 4096;
const MAX_DATABASE_NAME_BYTES: usize = 1024;

fn error_tag(code: &str) -> u8 {
    match code {
        "unauthorized" => 1,
        "conflict" => 2,
        "deadline" => 3,
        "response_too_large" => 4,
        _ => 5,
    }
}

fn tagged_error(tag: u8) -> Result<super::protocol::ServiceError> {
    Ok(match tag {
        1 => super::protocol::ServiceError::new("unauthorized", "Worker database access denied"),
        2 => super::protocol::ServiceError::new("conflict", "Worker database snapshot changed"),
        3 => super::protocol::ServiceError::new("deadline", "Worker database read timed out"),
        4 => super::protocol::ServiceError::new(
            "response_too_large",
            "Worker database snapshot exceeds the response limit",
        ),
        5 => super::protocol::ServiceError::new(
            "key_read_failed",
            "Worker database snapshot unavailable",
        ),
        _ => return Err(anyhow!("invalid worker database reply")),
    })
}

fn tagged_image_error(tag: u8) -> Result<super::protocol::ServiceError> {
    Ok(match tag {
        1 => super::protocol::ServiceError::new("unauthorized", "Worker image access denied"),
        2 => super::protocol::ServiceError::new("conflict", "Worker image snapshot changed"),
        3 => super::protocol::ServiceError::new("deadline", "Worker image read timed out"),
        4 => super::protocol::ServiceError::new(
            "response_too_large",
            "Worker image snapshot exceeds the response limit",
        ),
        5 => super::protocol::ServiceError::new(
            "key_read_failed",
            "Worker image snapshot unavailable",
        ),
        _ => return Err(anyhow!("invalid worker image reply")),
    })
}

fn put_bytes(output: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let length =
        u16::try_from(value.len()).map_err(|_| anyhow!("worker database field too large"))?;
    output.extend_from_slice(&length.to_le_bytes());
    output.extend_from_slice(value);
    Ok(())
}

pub(crate) fn encode_database_reply(
    runtime_id: &str,
    outcome: std::result::Result<DatabaseSnapshot, super::protocol::ServiceError>,
) -> Result<zeroize::Zeroizing<Vec<u8>>> {
    ensure!(
        runtime_id.len() <= u16::MAX as usize,
        "worker database runtime id too large"
    );
    let mut output = zeroize::Zeroizing::new(Vec::new());
    output.extend_from_slice(DATABASE_REPLY_MAGIC);
    put_bytes(&mut output, runtime_id.as_bytes())?;
    match outcome {
        Err(error) => {
            output.push(0);
            output.push(error_tag(&error.code));
        }
        Ok(snapshot) => {
            output.push(1);
            output.extend_from_slice(&snapshot.revision.to_le_bytes());
            match snapshot.databases {
                None => output.push(0),
                Some(keys) => {
                    ensure!(
                        keys.0.len() <= MAX_DATABASE_KEYS,
                        "too many worker database keys"
                    );
                    output.push(1);
                    output.extend_from_slice(&(keys.0.len() as u32).to_le_bytes());
                    let mut entries: Vec<_> = keys.0.iter().collect();
                    entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
                    for (name, key) in entries {
                        ensure!(
                            !name.is_empty()
                                && name.len() <= MAX_DATABASE_NAME_BYTES
                                && key.len() == 64
                                && key.bytes().all(|byte| byte.is_ascii_hexdigit()),
                            "invalid worker database material"
                        );
                        put_bytes(&mut output, name.as_bytes())?;
                        put_bytes(&mut output, key.as_bytes())?;
                    }
                }
            }
        }
    }
    ensure!(
        output.len() <= MAX_DATABASE_REPLY_BYTES,
        "worker database reply too large"
    );
    Ok(output)
}

struct DatabaseReplyReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> DatabaseReplyReader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| anyhow!("invalid worker database reply"))?;
        let value = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(value)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn field(&mut self, max: usize) -> Result<&'a [u8]> {
        let length = self.u16()? as usize;
        ensure!(length <= max, "invalid worker database reply");
        self.take(length)
    }

    fn text(&mut self, max: usize) -> Result<String> {
        String::from_utf8(self.field(max)?.to_vec())
            .map_err(|_| anyhow!("invalid worker database reply"))
    }
}

pub(crate) fn decode_database_reply(
    bytes: &[u8],
    expected_runtime_id: &str,
) -> Result<DatabaseSnapshot> {
    ensure!(
        bytes.len() <= MAX_DATABASE_REPLY_BYTES,
        "worker database reply too large"
    );
    let mut input = DatabaseReplyReader { bytes, offset: 0 };
    ensure!(
        input.take(DATABASE_REPLY_MAGIC.len())? == DATABASE_REPLY_MAGIC,
        "invalid worker database reply"
    );
    ensure!(
        input.text(u16::MAX as usize)? == expected_runtime_id,
        "worker database reply runtime mismatch"
    );
    match input.byte()? {
        0 => {
            let error = tagged_error(input.byte()?)?;
            ensure!(input.offset == bytes.len(), "invalid worker database reply");
            return Err(error.into());
        }
        1 => {}
        _ => return Err(anyhow!("invalid worker database reply")),
    }
    let revision = input.u64()?;
    let present = input.byte()?;
    let databases = match present {
        0 => None,
        1 => {
            let count = input.u32()? as usize;
            ensure!(count <= MAX_DATABASE_KEYS, "invalid worker database reply");
            let mut keys = DatabaseKeys(HashMap::with_capacity(count));
            for _ in 0..count {
                let name = std::str::from_utf8(input.field(MAX_DATABASE_NAME_BYTES)?)
                    .map_err(|_| anyhow!("invalid worker database reply"))?;
                let key = input.field(64)?;
                ensure!(
                    !name.is_empty()
                        && key.len() == 64
                        && key.iter().all(u8::is_ascii_hexdigit)
                        && !keys.0.contains_key(name),
                    "invalid worker database reply"
                );
                keys.0.insert(
                    name.to_owned(),
                    std::str::from_utf8(key)
                        .expect("ASCII hex is valid UTF-8")
                        .to_owned(),
                );
            }
            Some(keys)
        }
        _ => return Err(anyhow!("invalid worker database reply")),
    };
    ensure!(input.offset == bytes.len(), "invalid worker database reply");
    Ok(DatabaseSnapshot {
        revision,
        databases,
    })
}

pub(crate) fn encode_image_reply(
    runtime_id: &str,
    outcome: std::result::Result<ImageSnapshot, super::protocol::ServiceError>,
) -> Result<zeroize::Zeroizing<Vec<u8>>> {
    ensure!(
        runtime_id.len() <= u16::MAX as usize,
        "worker image runtime id too large"
    );
    let mut output = zeroize::Zeroizing::new(Vec::new());
    output.extend_from_slice(IMAGE_REPLY_MAGIC);
    put_bytes(&mut output, runtime_id.as_bytes())?;
    match outcome {
        Err(error) => {
            output.push(0);
            output.push(error_tag(&error.code));
        }
        Ok(snapshot) => {
            output.push(1);
            output.extend_from_slice(&snapshot.revision.to_le_bytes());
            match snapshot.material {
                None => output.push(0),
                Some(material) => {
                    output.push(1);
                    output.extend_from_slice(&material.aes);
                    output.push(material.xor);
                }
            }
        }
    }
    ensure!(
        output.len() <= MAX_IMAGE_REPLY_BYTES,
        "worker image reply too large"
    );
    Ok(output)
}

pub(crate) fn decode_image_reply(bytes: &[u8], expected_runtime_id: &str) -> Result<ImageSnapshot> {
    ensure!(
        bytes.len() <= MAX_IMAGE_REPLY_BYTES,
        "worker image reply too large"
    );
    let mut input = DatabaseReplyReader { bytes, offset: 0 };
    ensure!(
        input.take(IMAGE_REPLY_MAGIC.len())? == IMAGE_REPLY_MAGIC,
        "invalid worker image reply"
    );
    ensure!(
        input.text(u16::MAX as usize)? == expected_runtime_id,
        "worker image reply runtime mismatch"
    );
    match input.byte()? {
        0 => {
            let error = tagged_image_error(input.byte()?)?;
            ensure!(input.offset == bytes.len(), "invalid worker image reply");
            return Err(error.into());
        }
        1 => {}
        _ => return Err(anyhow!("invalid worker image reply")),
    }
    let revision = input.u64()?;
    let material = match input.byte()? {
        0 => None,
        1 => Some(ImageMaterial {
            aes: input.take(16)?.try_into().unwrap(),
            xor: input.byte()?,
        }),
        _ => return Err(anyhow!("invalid worker image reply")),
    };
    ensure!(input.offset == bytes.len(), "invalid worker image reply");
    Ok(ImageSnapshot { revision, material })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Access {
    pub runtime: RuntimeContext,
    pub parent: client::ProcessIdentity,
    pub capability: Secret,
    pub revision: u64,
    pub image: Option<ImageMaterial>,
    pub initialization: Option<InitSeed>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input<T> {
    pub operation: T,
    pub access: Option<Access>,
}

static ACCESS: Mutex<Option<Access>> = Mutex::new(None);
// Updated only under ACCESS: odd means installed, even means no worker scope.
// The generation also prevents an old in-flight reply from updating a new scope.
static SCOPE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
#[must_use = "keep the guard alive for the entire worker operation"]
pub struct Guard {
    scope: u64,
}

pub fn install(access: Option<Access>) -> Result<Guard> {
    let mut slot = ACCESS
        .lock()
        .map_err(|_| anyhow!("worker key access unavailable"))?;
    ensure!(
        SCOPE.load(Ordering::Relaxed) & 1 == 0,
        "worker key access already installed"
    );
    *slot = access;
    let scope = SCOPE.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    Ok(Guard { scope })
}

impl Drop for Guard {
    fn drop(&mut self) {
        let mut slot = ACCESS.lock().unwrap_or_else(|error| error.into_inner());
        if SCOPE.load(Ordering::Relaxed) == self.scope {
            *slot = None;
            SCOPE.fetch_add(1, Ordering::Relaxed);
        }
    }
}

fn snapshot(runtime: &RuntimeContext) -> Result<(Access, u64)> {
    let (access, scope) = {
        let slot = ACCESS
            .lock()
            .map_err(|_| anyhow!("worker key access unavailable"))?;
        let access = slot
            .as_ref()
            .ok_or_else(|| anyhow!("worker key access not installed"))?;
        (access.clone(), SCOPE.load(Ordering::Relaxed))
    };
    ensure!(
        !runtime.is_bootstrap() && !access.runtime.is_bootstrap(),
        "bootstrap worker key access unsupported"
    );
    let matches = runtime
        .same_account(&access.runtime)
        .map_err(|_| anyhow!("worker key runtime validation failed"))?;
    ensure!(matches, "worker key runtime mismatch");
    Ok((access, scope))
}

pub fn expected_revision(runtime: &RuntimeContext) -> Result<u64> {
    Ok(snapshot(runtime)?.0.revision)
}

pub fn initialization_seed(runtime: &RuntimeContext) -> Result<InitSeed> {
    let seed = snapshot(runtime)?
        .0
        .initialization
        .ok_or_else(|| anyhow!("worker initialization access not installed"))?;
    if let Some(account) = &seed.account {
        ensure!(
            account.as_bytes().len() == 32,
            "invalid account key material"
        );
    }
    Ok(seed)
}

pub fn database_keys(runtime: &RuntimeContext) -> Result<Option<DatabaseKeys>> {
    let (access, scope) = snapshot(runtime)?;
    let result = block_on(client::request_database_keys(
        &access.runtime,
        DatabaseReadRequest {
            capability: access.capability.clone(),
        },
        &access.parent,
    ))?;
    let current = ACCESS
        .lock()
        .map_err(|_| anyhow!("worker key access unavailable"))?;
    ensure!(
        SCOPE.load(Ordering::Relaxed) == scope
            && current.as_ref().is_some_and(|current| {
                current.capability.as_str() == access.capability.as_str()
            }),
        "worker database scope changed"
    );
    Ok(result.databases)
}

pub fn image_material(runtime: &RuntimeContext) -> Result<Option<ImageMaterial>> {
    let (access, scope) = snapshot(runtime)?;
    if access.image.is_none() {
        let result = block_on(client::request_image_material(
            &access.runtime,
            ImageReadRequest {
                capability: access.capability.clone(),
            },
            &access.parent,
        ))?;
        let mut current = ACCESS
            .lock()
            .map_err(|_| anyhow!("worker key access unavailable"))?;
        ensure!(
            SCOPE.load(Ordering::Relaxed) == scope
                && current.as_ref().is_some_and(|current| {
                    current.capability.as_str() == access.capability.as_str()
                }),
            "worker image scope changed"
        );
        let current = current.as_mut().expect("worker scope checked above");
        current.revision = result.revision;
        current.image = result.material;
        return Ok(current.image.clone());
    }
    verify_image_access(&access, scope)?;
    Ok(access.image)
}

pub fn verify_image_revision(runtime: &RuntimeContext) -> Result<()> {
    let (access, scope) = snapshot(runtime)?;
    verify_image_access(&access, scope)
}

fn image_read_error(error: anyhow::Error) -> anyhow::Error {
    use super::protocol::ServiceError;
    let (code, message) = match error
        .downcast_ref::<ServiceError>()
        .map(|error| error.code.as_str())
    {
        Some("conflict") => ("conflict", "Worker image snapshot changed"),
        Some("unauthorized") => ("unauthorized", "Worker image access denied"),
        _ => (
            "key_read_failed",
            "Worker image snapshot could not be verified",
        ),
    };
    ServiceError::new(code, message).into()
}

fn verify_image_access(access: &Access, scope: u64) -> Result<()> {
    block_on(async {
        let reply = client::request_bound(
            &access.runtime,
            Call::WorkerKeyRevision {
                request: RevisionRequest {
                    capability: access.capability.clone(),
                    expected_revision: access.revision,
                },
            },
            &access.parent,
        )
        .await
        .map_err(image_read_error)?;
        ensure!(
            reply.get("verified").and_then(serde_json::Value::as_bool) == Some(true),
            "Invalid worker image verification reply"
        );
        Ok(())
    })?;
    let current = ACCESS
        .lock()
        .map_err(|_| anyhow!("worker key access unavailable"))?;
    ensure!(
        SCOPE.load(Ordering::Relaxed) == scope
            && current
                .as_ref()
                .is_some_and(|current| current.revision == access.revision),
        "worker image scope or revision changed"
    );
    Ok(())
}

fn request_error(error: anyhow::Error) -> anyhow::Error {
    use super::protocol::ServiceError;

    // Preserve definite broker rejections without forwarding its message or
    // source chain. Transport failures may have occurred after the commit.
    let (code, message) = match error
        .downcast_ref::<ServiceError>()
        .map(|error| error.code.as_str())
    {
        Some("conflict") => ("conflict", "Worker key revision changed"),
        Some("busy") => ("busy", "Worker key update is busy"),
        Some("unauthorized") => ("unauthorized", "Worker key access denied"),
        Some("invalid_request") => ("invalid_request", "Invalid worker key update"),
        _ => (
            "outcome_unknown",
            "Worker key update outcome is unknown; do not retry automatically",
        ),
    };
    ServiceError::new(code, message).into()
}

pub async fn commit(runtime: &RuntimeContext, changes: Vec<MaterialChange>) -> Result<u64> {
    let (access, scope) = snapshot(runtime)?;
    commit_access(access, scope, changes).await
}

async fn commit_access(access: Access, scope: u64, changes: Vec<MaterialChange>) -> Result<u64> {
    let image = changes.iter().rev().find_map(|change| match change {
        MaterialChange::Image { aes, xor } => Some(ImageMaterial {
            aes: *aes,
            xor: *xor,
        }),
        _ => None,
    });
    let request = UpdateRequest {
        capability: access.capability.clone(),
        expected_revision: access.revision,
        changes,
    };
    let result = client::request_bound(
        &access.runtime,
        Call::WorkerKeys { request },
        &access.parent,
    )
    .await
    .map_err(request_error)?;
    let revision = result
        .get("revision")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("invalid worker key revision reply; outcome may be unknown"))?;
    let mut slot = ACCESS
        .lock()
        .map_err(|_| anyhow!("worker key access unavailable; outcome may be unknown"))?;
    ensure!(
        SCOPE.load(Ordering::Relaxed) == scope,
        "worker key scope changed; outcome may be unknown"
    );
    let current = slot
        .as_mut()
        .ok_or_else(|| anyhow!("worker key access not installed; outcome may be unknown"))?;
    ensure!(
        current.revision == access.revision,
        "worker key revision changed; outcome may be unknown"
    );
    current.revision = revision;
    if let Some(image) = image {
        current.image = Some(image);
    }
    Ok(revision)
}

pub fn commit_sync(runtime: &RuntimeContext, changes: Vec<MaterialChange>) -> Result<u64> {
    block_on(commit(runtime, changes))
}

pub(crate) fn verify_image_import_revision(
    runtime: &RuntimeContext,
    expected_revision: u64,
) -> Result<()> {
    let (access, scope) = snapshot(runtime)?;
    ensure!(
        access.revision == expected_revision,
        super::protocol::ServiceError::new("conflict", "Image import material revision changed")
    );
    verify_image_access(&access, scope)
}

pub(crate) fn commit_image_import_sync(
    runtime: &RuntimeContext,
    expected_revision: u64,
    material: &ImageMaterial,
) -> Result<u64> {
    let (access, scope) = snapshot(runtime)?;
    ensure!(
        access.revision == expected_revision,
        super::protocol::ServiceError::new("conflict", "Image import material revision changed")
    );
    block_on(commit_access(
        access,
        scope,
        vec![MaterialChange::Image {
            aes: material.aes,
            xor: material.xor,
        }],
    ))
}

fn block_on<T: Send>(future: impl std::future::Future<Output = Result<T>> + Send) -> Result<T> {
    // A fresh thread avoids nested block_on when synchronous business code is
    // already running inside Tokio. request supplies the existing pipe deadlines.
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .spawn_scoped(scope, move || {
                let executor = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| anyhow!("worker key executor unavailable"))?;
                executor.block_on(future)
            })
            .map_err(|_| anyhow!("worker key thread unavailable"))?;
        worker
            .join()
            .map_err(|_| anyhow!("worker key thread failed; outcome may be unknown"))?
    })
}

#[cfg(test)]
thread_local! {
    static DROPS: std::cell::RefCell<Vec<bool>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
fn observe_drop(wiped: bool) {
    DROPS.with(|events| events.borrow_mut().push(wiped));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        service::{operations::Operation, plan::Step},
    };

    fn runtime() -> RuntimeContext {
        let root = std::env::temp_dir().join("wx-worker-keys-synthetic");
        RuntimeContext {
            config: Config {
                key_store: None,
                db_dir: root.join("db"),
                keys_file: root.join("keys"),
                decrypted_dir: root.join("cache"),
                wechat_process: "synthetic.exe".into(),
            },
            config_path: root.join("config.json"),
            directory: root.join("accounts/synthetic"),
            id: "synthetic".into(),
            root,
        }
    }

    fn access() -> Access {
        Access {
            runtime: runtime(),
            parent: client::ProcessIdentity {
                pid: 41,
                created: 101,
            },
            capability: Secret::new("synthetic-capability"),
            revision: 7,
            image: None,
            initialization: None,
        }
    }

    fn raw_database_reply(runtime_id: &str, entries: &[(&str, &str)]) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(DATABASE_REPLY_MAGIC);
        put_bytes(&mut output, runtime_id.as_bytes()).unwrap();
        output.push(1);
        output.extend_from_slice(&17_u64.to_le_bytes());
        output.push(1);
        output.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for (name, key) in entries {
            put_bytes(&mut output, name.as_bytes()).unwrap();
            put_bytes(&mut output, key.as_bytes()).unwrap();
        }
        output
    }

    #[test]
    fn database_reply_round_trips_absent_and_present_snapshots() {
        let absent = encode_database_reply(
            "synthetic-runtime",
            Ok(DatabaseSnapshot {
                revision: 11,
                databases: None,
            }),
        )
        .unwrap();
        let decoded = decode_database_reply(&absent, "synthetic-runtime").unwrap();
        assert_eq!(decoded.revision, 11);
        assert!(decoded.databases.is_none());

        let present = encode_database_reply(
            "synthetic-runtime",
            Ok(DatabaseSnapshot {
                revision: 12,
                databases: Some(DatabaseKeys(HashMap::from([
                    ("message/msg0.db".into(), "ab".repeat(32)),
                    ("contact/contact.db".into(), "01".repeat(32)),
                ]))),
            }),
        )
        .unwrap();
        let decoded = decode_database_reply(&present, "synthetic-runtime").unwrap();
        assert_eq!(decoded.revision, 12);
        let keys = decoded.databases.unwrap();
        assert_eq!(keys.0["message/msg0.db"], "ab".repeat(32));
        assert_eq!(keys.0["contact/contact.db"], "01".repeat(32));
    }

    #[test]
    fn database_reply_accepts_4096_keys_and_rejects_more() {
        let at_limit = (0..MAX_DATABASE_KEYS)
            .map(|index| (format!("message/msg{index:04}.db"), "cd".repeat(32)))
            .collect::<HashMap<_, _>>();
        let wire = encode_database_reply(
            "synthetic-runtime",
            Ok(DatabaseSnapshot {
                revision: 19,
                databases: Some(DatabaseKeys(at_limit)),
            }),
        )
        .unwrap();
        let decoded = decode_database_reply(&wire, "synthetic-runtime").unwrap();
        assert_eq!(decoded.databases.unwrap().0.len(), MAX_DATABASE_KEYS);

        let over_limit = (0..=MAX_DATABASE_KEYS)
            .map(|index| (format!("message/msg{index:04}.db"), "ef".repeat(32)))
            .collect::<HashMap<_, _>>();
        assert!(encode_database_reply(
            "synthetic-runtime",
            Ok(DatabaseSnapshot {
                revision: 20,
                databases: Some(DatabaseKeys(over_limit)),
            }),
        )
        .is_err());
    }

    #[test]
    fn database_reply_rejects_duplicate_names() {
        let key = "12".repeat(32);
        let wire = raw_database_reply(
            "synthetic-runtime",
            &[("message/msg0.db", &key), ("message/msg0.db", &key)],
        );
        assert!(decode_database_reply(&wire, "synthetic-runtime").is_err());
    }

    #[test]
    fn database_reply_rejects_every_truncation_and_trailing_bytes() {
        let key = "34".repeat(32);
        let wire = raw_database_reply("synthetic-runtime", &[("message/msg0.db", &key)]);
        for end in 0..wire.len() {
            assert!(
                decode_database_reply(&wire[..end], "synthetic-runtime").is_err(),
                "truncated reply unexpectedly decoded at byte {end}"
            );
        }
        assert!(decode_database_reply(&wire, "synthetic-runtime").is_ok());

        let mut trailing = wire;
        trailing.push(0);
        assert!(decode_database_reply(&trailing, "synthetic-runtime").is_err());
    }

    #[test]
    fn database_reply_rejects_non_hex_keys() {
        let invalid = "g0".repeat(32);
        let wire = raw_database_reply("synthetic-runtime", &[("message/msg0.db", &invalid)]);
        assert!(decode_database_reply(&wire, "synthetic-runtime").is_err());
    }

    #[test]
    fn database_reply_rejects_unknown_status_and_error_tags() {
        let mut success = raw_database_reply("synthetic-runtime", &[]);
        let status = DATABASE_REPLY_MAGIC.len() + 2 + "synthetic-runtime".len();
        success[status] = 2;
        assert!(decode_database_reply(&success, "synthetic-runtime").is_err());

        let mut error = DATABASE_REPLY_MAGIC.to_vec();
        put_bytes(&mut error, b"synthetic-runtime").unwrap();
        error.extend_from_slice(&[0, 99]);
        assert!(decode_database_reply(&error, "synthetic-runtime").is_err());
    }

    #[test]
    fn image_reply_round_trips_absent_present_and_typed_error() {
        let absent = encode_image_reply(
            "synthetic-runtime",
            Ok(ImageSnapshot {
                revision: 31,
                material: None,
            }),
        )
        .unwrap();
        let decoded = decode_image_reply(&absent, "synthetic-runtime").unwrap();
        assert_eq!(decoded.revision, 31);
        assert!(decoded.material.is_none());

        let present = encode_image_reply(
            "synthetic-runtime",
            Ok(ImageSnapshot {
                revision: 32,
                material: Some(ImageMaterial {
                    aes: [0xa5; 16],
                    xor: 0x67,
                }),
            }),
        )
        .unwrap();
        let decoded = decode_image_reply(&present, "synthetic-runtime").unwrap();
        assert_eq!(decoded.revision, 32);
        let material = decoded.material.unwrap();
        assert_eq!(material.aes, [0xa5; 16]);
        assert_eq!(material.xor, 0x67);

        let denied = encode_image_reply(
            "synthetic-runtime",
            Err(super::super::protocol::ServiceError::unauthorized()),
        )
        .unwrap();
        let error = decode_image_reply(&denied, "synthetic-runtime").unwrap_err();
        assert_eq!(
            error
                .downcast_ref::<super::super::protocol::ServiceError>()
                .unwrap()
                .code,
            "unauthorized"
        );
    }

    #[test]
    fn image_reply_rejects_wrong_runtime_every_truncation_and_trailing_bytes() {
        let wire = encode_image_reply(
            "synthetic-runtime",
            Ok(ImageSnapshot {
                revision: 33,
                material: Some(ImageMaterial {
                    aes: [0xb6; 16],
                    xor: 0x78,
                }),
            }),
        )
        .unwrap();
        assert!(decode_image_reply(&wire, "other-runtime").is_err());
        for end in 0..wire.len() {
            assert!(
                decode_image_reply(&wire[..end], "synthetic-runtime").is_err(),
                "truncated reply unexpectedly decoded at byte {end}"
            );
        }
        assert!(decode_image_reply(&wire, "synthetic-runtime").is_ok());
        let mut trailing = wire.to_vec();
        trailing.push(0);
        assert!(decode_image_reply(&trailing, "synthetic-runtime").is_err());
    }

    #[test]
    fn image_reply_rejects_unknown_status_presence_and_error_tags() {
        let wire = encode_image_reply(
            "synthetic-runtime",
            Ok(ImageSnapshot {
                revision: 34,
                material: None,
            }),
        )
        .unwrap();
        let status = IMAGE_REPLY_MAGIC.len() + 2 + "synthetic-runtime".len();
        let mut unknown_status = wire.to_vec();
        unknown_status[status] = 2;
        assert!(decode_image_reply(&unknown_status, "synthetic-runtime").is_err());
        let mut unknown_presence = wire.to_vec();
        unknown_presence[status + 1 + size_of::<u64>()] = 2;
        assert!(decode_image_reply(&unknown_presence, "synthetic-runtime").is_err());

        let mut error = IMAGE_REPLY_MAGIC.to_vec();
        put_bytes(&mut error, b"synthetic-runtime").unwrap();
        error.extend_from_slice(&[0, 99]);
        assert!(decode_image_reply(&error, "synthetic-runtime").is_err());
    }

    #[test]
    fn database_read_types_redact_secrets_from_debug_output() {
        let request = DatabaseReadRequest {
            capability: Secret::new("database-read-secret"),
        };
        let snapshot = DatabaseSnapshot {
            revision: 23,
            databases: Some(DatabaseKeys(HashMap::from([(
                "private-database-name".into(),
                "56".repeat(32),
            )]))),
        };

        let request_debug = format!("{request:?}");
        let snapshot_debug = format!("{snapshot:?}");
        assert!(!request_debug.contains("database-read-secret"));
        assert!(!snapshot_debug.contains("private-database-name"));
        assert!(!snapshot_debug.contains(&"56".repeat(32)));
        assert!(snapshot_debug.contains("[REDACTED]"));
    }

    #[test]
    fn access_serialization_does_not_embed_database_keys() {
        let value = serde_json::to_value(access()).unwrap();
        let object = value.as_object().unwrap();
        assert!(!object.contains_key("databases"));
        assert_eq!(
            object
                .keys()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from([
                "capability",
                "image",
                "initialization",
                "parent",
                "revision",
                "runtime",
            ])
        );
    }

    #[test]
    fn database_snapshot_is_redacted_and_wiped_after_borrowing() {
        DROPS.with(|events| events.borrow_mut().clear());
        let keys = DatabaseKeys(HashMap::from([(
            "contact/contact.db".into(),
            "41".repeat(32),
        )]));
        assert_eq!(format!("{keys:?}"), "DatabaseKeys([REDACTED])");
        assert_eq!(keys.0.get("contact/contact.db").unwrap().len(), 64);
        drop(keys);
        DROPS.with(|events| assert_eq!(*events.borrow(), vec![true]));
    }

    #[test]
    fn image_snapshot_material_is_redacted_and_wiped() {
        DROPS.with(|events| events.borrow_mut().clear());
        let material = ImageMaterial {
            aes: [71; 16],
            xor: 53,
        };
        assert_eq!(format!("{material:?}"), "ImageMaterial([REDACTED])");
        drop(material);
        DROPS.with(|events| assert_eq!(*events.borrow(), vec![true]));

        let request = ImageReadRequest {
            capability: Secret::new("image-read-secret"),
        };
        let snapshot = ImageSnapshot {
            revision: 41,
            material: Some(ImageMaterial {
                aes: [72; 16],
                xor: 54,
            }),
        };
        assert!(!format!("{request:?}").contains("image-read-secret"));
        let debug = format!("{snapshot:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("72"));
        assert!(!debug.contains("54"));
    }

    #[test]
    fn image_verification_errors_preserve_codes_without_private_details() {
        use super::super::protocol::ServiceError;
        for code in ["conflict", "unauthorized", "key_read_failed"] {
            let error = image_read_error(ServiceError::new(code, "private material").into());
            assert_eq!(error.downcast_ref::<ServiceError>().unwrap().code, code);
            assert!(!format!("{error:#}").contains("private material"));
        }
        let error = image_read_error(anyhow!("private transport detail"));
        assert_eq!(
            error.downcast_ref::<ServiceError>().unwrap().code,
            "key_read_failed"
        );
        assert!(!format!("{error:#}").contains("private transport"));
    }

    #[test]
    fn request_errors_preserve_rejections_without_leaking_source_messages() {
        use super::super::protocol::ServiceError;

        for code in ["conflict", "busy", "unauthorized", "invalid_request"] {
            let source = anyhow::Error::new(ServiceError::new(code, "synthetic-secret"))
                .context("synthetic-secret context");
            let error = request_error(source);
            assert_eq!(error.downcast_ref::<ServiceError>().unwrap().code, code);
            assert!(!format!("{error:#}").contains("synthetic-secret"));
            assert!(!format!("{error:?}").contains("synthetic-secret"));
        }
        for source in [
            anyhow!("synthetic-secret transport failure"),
            ServiceError::new("key_update_failed", "synthetic-secret").into(),
            ServiceError::new("synthetic-secret unknown code", "synthetic-secret").into(),
        ] {
            let error = request_error(source);
            assert_eq!(
                error.downcast_ref::<ServiceError>().unwrap().code,
                "outcome_unknown"
            );
            assert!(!format!("{error:#}").contains("synthetic-secret"));
            assert!(!format!("{error:?}").contains("synthetic-secret"));
        }
    }

    #[test]
    fn private_wire_round_trip_and_debug_redaction() {
        let request = UpdateRequest {
            capability: Secret::new("synthetic-capability"),
            expected_revision: 7,
            changes: vec![
                MaterialChange::Account(vec![93; 32]),
                MaterialChange::Databases(HashMap::from([(
                    "synthetic-db".into(),
                    "synthetic-key".into(),
                )])),
                MaterialChange::Image {
                    aes: [94; 16],
                    xor: 95,
                },
            ],
        };
        let call = Call::WorkerKeys { request };
        let debug = format!("{:?}", call.clone());
        for secret in [
            "synthetic-capability",
            "synthetic-db",
            "synthetic-key",
            "93",
            "94",
            "95",
        ] {
            assert!(!debug.contains(secret));
        }
        assert_eq!(
            call.response_limit(),
            super::super::protocol::MAX_RESPONSE_BYTES
        );
        let wire = zeroize::Zeroizing::new(serde_json::to_vec(&call).unwrap());
        let decoded: Call = serde_json::from_slice(&wire).unwrap();
        let Call::WorkerKeys { request } = decoded else {
            panic!("wrong call")
        };
        assert_eq!(request.capability.as_str(), "synthetic-capability");
        assert_eq!(request.expected_revision, 7);
        assert!(
            matches!(&request.changes[0], MaterialChange::Account(bytes) if bytes == &vec![93; 32])
        );
        assert!(
            matches!(&request.changes[1], MaterialChange::Databases(keys) if keys["synthetic-db"] == "synthetic-key")
        );
        assert!(
            matches!(&request.changes[2], MaterialChange::Image { aes, xor } if aes == &[94; 16] && *xor == 95)
        );
    }

    #[test]
    fn verification_and_unknown_fields_are_rejected() {
        for wire in [
            r#"{"kind":"account","material":[1],"verified":true}"#,
            r#"{"kind":"image","material":{"aes":[0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"xor":0,"verified":true}}"#,
        ] {
            assert!(serde_json::from_str::<MaterialChange>(wire).is_err());
        }
        assert!(serde_json::from_str::<UpdateRequest>(
            r#"{"capability":"synthetic","expected_revision":0,"changes":[],"verified":true}"#
        )
        .is_err());
    }

    #[test]
    fn drops_observe_wiping_before_storage_is_released() {
        DROPS.with(|events| events.borrow_mut().clear());
        drop(UpdateRequest {
            capability: Secret::new("synthetic"),
            expected_revision: 0,
            changes: vec![
                MaterialChange::Account(vec![1; 32]),
                MaterialChange::Databases(HashMap::from([(
                    "synthetic-db".into(),
                    "synthetic-key".into(),
                )])),
                MaterialChange::Image {
                    aes: [2; 16],
                    xor: 3,
                },
            ],
        });
        DROPS.with(|events| assert_eq!(*events.borrow(), vec![true; 4]));
    }

    #[test]
    fn input_reuses_operation_and_step_contracts() {
        let operation = Input {
            operation: Operation::Extract {
                attachment_id: "synthetic".into(),
                output: "synthetic".into(),
                overwrite: false,
                json: false,
            },
            access: Some(access()),
        };
        assert!(!format!("{operation:?}").contains("synthetic-capability"));
        let wire = zeroize::Zeroizing::new(serde_json::to_vec(&operation).unwrap());
        let decoded: Input<Operation> = serde_json::from_slice(&wire).unwrap();
        let decoded_access = decoded.access.unwrap();
        assert_eq!(decoded_access.parent, access().parent);
        assert_eq!(decoded_access.capability.as_str(), "synthetic-capability");
        let mut missing_parent = serde_json::to_value(access()).unwrap();
        missing_parent.as_object_mut().unwrap().remove("parent");
        assert!(serde_json::from_value::<Access>(missing_parent).is_err());
        let step = Input {
            operation: Step::WechatDecrypt {
                config: "synthetic.json".into(),
            },
            access: None,
        };
        let wire = serde_json::to_vec(&step).unwrap();
        let decoded: Input<Step> = serde_json::from_slice(&wire).unwrap();
        assert!(decoded.access.is_none());
    }

    #[test]
    fn scope_rejects_missing_access_mismatch_and_nested_install() {
        let runtime = runtime();
        assert!(expected_revision(&runtime).is_err());
        let empty = install(None).unwrap();
        assert!(expected_revision(&runtime).is_err());
        assert!(install(Some(access())).is_err());
        drop(empty);
        let guard = install(Some(access())).unwrap();
        assert_eq!(expected_revision(&runtime).unwrap(), 7);
        let mut mismatch = runtime.clone();
        mismatch.id = "other-account".into();
        assert!(expected_revision(&mismatch).is_err());
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        executor.block_on(async {
            assert!(commit_sync(&mismatch, vec![]).is_err());
        });
        drop(guard);
        assert!(expected_revision(&runtime).is_err());
        assert!(commit_sync(&runtime, vec![]).is_err());
    }
}
