//! Private, process-bound access to the daemon's key update transaction.
use super::query_state::{KeyChange, QueryState};
use crate::{
    runtime::RuntimeContext,
    service::{
        operations::Operation,
        plan::Step,
        protocol::ServiceError,
        worker_keys::{
            Access, AccountMaterial, DatabaseKeys, DatabaseReadRequest, DatabaseSnapshot,
            ImageMaterial, ImageReadRequest, ImageSnapshot, InitSeed, MaterialChange,
            RevisionRequest, Secret, UpdateRequest,
        },
    },
};
use anyhow::{ensure, Result};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    os::windows::io::{AsRawHandle, BorrowedHandle, OwnedHandle},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use windows::Win32::{
    Foundation::{HANDLE, WAIT_TIMEOUT},
    System::Threading::WaitForSingleObject,
};

const ACCOUNT: u8 = 1;
const DATABASES: u8 = 2;
const IMAGE: u8 = 4;
const READ_IMAGE: u8 = 8;
const INIT_MEMORY: u8 = 16;
const READ_DATABASES: u8 = 32;
const PRELOAD_IMAGE: u8 = 64;
const INIT_SAVED: u8 = 128;
const MAX_GRANTS: usize = 64;
type DigestBytes = [u8; 32];

#[cfg(test)]
#[path = "worker_keys_tests.rs"]
mod tests;

struct Grant {
    active: Arc<AtomicBool>,
    process: OwnedHandle,
    pid: u32,
    permissions: u8,
    revision: u64,
    completed: Option<(DigestBytes, u64)>,
}

impl Grant {
    fn belongs_to_live_process(&self, peer_pid: u32) -> bool {
        self.active.load(Ordering::Acquire)
            && self.pid == peer_pid
            && unsafe { WaitForSingleObject(HANDLE(self.process.as_raw_handle()), 0) }
                == WAIT_TIMEOUT
    }
}

pub(crate) struct Broker {
    runtime: RuntimeContext,
    query: Arc<QueryState>,
    grants: Mutex<HashMap<DigestBytes, Arc<tokio::sync::Mutex<Grant>>>>,
    closing: AtomicBool,
}

pub(crate) struct Registration {
    broker: Arc<Broker>,
    id: DigestBytes,
    active: Arc<AtomicBool>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        if let Ok(mut grants) = self.broker.grants.lock() {
            grants.remove(&self.id);
        }
    }
}

fn digest(bytes: &[u8]) -> DigestBytes {
    Sha256::digest(bytes).into()
}

fn update_signature(request: &UpdateRequest) -> DigestBytes {
    fn field(hash: &mut Sha256, bytes: &[u8]) {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    let mut hash = Sha256::new();
    hash.update(request.expected_revision.to_le_bytes());
    hash.update((request.changes.len() as u64).to_le_bytes());
    for change in &request.changes {
        hash.update([change_permission(change)]);
        match change {
            MaterialChange::Account(key) => field(&mut hash, key),
            MaterialChange::Databases(keys) => {
                // A retry is independent of each deserialization's HashMap seed.
                let mut entries: Vec<_> = keys.iter().collect();
                entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
                hash.update((entries.len() as u64).to_le_bytes());
                for (name, key) in entries {
                    field(&mut hash, name.as_bytes());
                    field(&mut hash, key.as_bytes());
                }
            }
            MaterialChange::Image { aes, xor } => {
                hash.update(aes);
                hash.update([*xor]);
            }
        }
    }
    hash.finalize().into()
}

fn permissions(operation: &Operation) -> u8 {
    match operation {
        Operation::Voices { .. } | Operation::TranscribeBatch { .. } => READ_DATABASES,
        Operation::ExportAll { args }
            if !args.dry_run && args.with_transcriptions && args.write_plan_csv.is_none() =>
        {
            READ_DATABASES
        }
        Operation::ExportMessages { args } if !args.dry_run && !args.no_media => {
            READ_DATABASES | READ_IMAGE | PRELOAD_IMAGE
        }
        Operation::Toolkit {
            operation:
                crate::service::operations::ToolkitOperation::Decrypt { .. }
                | crate::service::operations::ToolkitOperation::ExportEmoticons(_),
        } => READ_DATABASES,
        Operation::Toolkit {
            operation:
                crate::service::operations::ToolkitOperation::DecodeImages { .. }
                | crate::service::operations::ToolkitOperation::DecodeImage { .. }
                | crate::service::operations::ToolkitOperation::BatchDecryptImages { .. },
        } => READ_IMAGE,
        Operation::DatabaseKeys { args } if args.authorize_memory_scan => DATABASES,
        Operation::ImageKeys { args }
            if !args.no_save && (args.offline != args.authorize_memory_scan) =>
        {
            IMAGE
        }
        Operation::ImageKeyMonitor { args } if args.authorize_memory_scan => {
            READ_IMAGE | PRELOAD_IMAGE | if args.saves_keys() { IMAGE } else { 0 }
        }
        Operation::SnsArchive { .. } => READ_IMAGE | PRELOAD_IMAGE,
        Operation::SnsTimeline { .. } => READ_IMAGE,
        _ => 0,
    }
}

fn change_permission(change: &MaterialChange) -> u8 {
    match change {
        MaterialChange::Account(_) => ACCOUNT,
        MaterialChange::Databases(_) => DATABASES,
        MaterialChange::Image { .. } => IMAGE,
    }
}

fn update_error(error: anyhow::Error) -> ServiceError {
    use crate::key_store::Error;
    match error.downcast_ref::<Error>() {
        Some(Error::Conflict) => ServiceError::new("conflict", "Key revision changed"),
        Some(Error::Busy) => ServiceError::new("busy", "Key update is busy"),
        _ => ServiceError::new(
            "key_update_failed",
            "Key update failed; no material is returned",
        ),
    }
}

impl Broker {
    pub fn new(runtime: RuntimeContext, query: Arc<QueryState>) -> Arc<Self> {
        Arc::new(Self {
            runtime,
            query,
            grants: Mutex::new(HashMap::new()),
            closing: AtomicBool::new(false),
        })
    }

    pub async fn register(
        self: &Arc<Self>,
        child: &tokio::process::Child,
        operation: &Operation,
    ) -> Result<Option<(Access, Registration)>> {
        if let Operation::Initialize {
            force,
            provider,
            db_dir_override,
            restart,
            executable,
            ..
        } = operation
        {
            let permissions = match provider {
                crate::scanner::KeyProvider::Memory if !restart && executable.is_none() => {
                    DATABASES | INIT_MEMORY
                }
                crate::scanner::KeyProvider::Saved if !restart && executable.is_none() => {
                    DATABASES | INIT_SAVED
                }
                crate::scanner::KeyProvider::Account if *force && *restart => ACCOUNT | DATABASES,
                _ => return Ok(None),
            };
            if self.runtime.is_bootstrap() {
                return Ok(None);
            }
            let document = crate::infrastructure::configuration::ConfigDocument::load(
                &self.runtime.config_path,
            )?;
            let database = db_dir_override.as_ref().map_or_else(
                || self.runtime.config.db_dir.clone(),
                std::path::PathBuf::from,
            );
            let configuration = document.with_db(&database)?;
            if configuration == document.value {
                return self.register_permissions(child, permissions).await;
            }
            return Ok(None);
        }
        self.register_permissions(child, permissions(operation))
            .await
    }

    pub async fn register_step(
        self: &Arc<Self>,
        child: &tokio::process::Child,
        step: &Step,
    ) -> Result<Option<(Access, Registration)>> {
        let (config, permissions) = match step {
            Step::WechatKeys {
                config,
                authorize_memory_scan: true,
            } => (config, DATABASES),
            Step::ImageKey {
                config,
                authorize_memory_scan: true,
                ..
            } => (config, IMAGE),
            Step::WechatDecrypt { config } => (config, READ_DATABASES),
            Step::DecodeImages { config, .. } => (config, READ_IMAGE),
            Step::ExportMessages {
                config,
                include_images: true,
                ..
            } => (config, READ_DATABASES | READ_IMAGE | PRELOAD_IMAGE),
            Step::TranscribeChats { config, .. } => (config, READ_DATABASES),
            Step::SnsArchive { config, .. } => (config, READ_IMAGE | PRELOAD_IMAGE),
            _ => return Ok(None),
        };
        ensure!(
            config == &self.runtime.config_path,
            "Worker configuration mismatch"
        );
        self.register_permissions(child, permissions).await
    }

    async fn register_permissions(
        self: &Arc<Self>,
        child: &tokio::process::Child,
        permissions: u8,
    ) -> Result<Option<(Access, Registration)>> {
        if permissions == 0 || self.runtime.is_bootstrap() {
            return Ok(None);
        }
        ensure!(
            !self.closing.load(Ordering::Acquire),
            "Worker key service is stopping"
        );
        let pid = child
            .id()
            .ok_or_else(|| anyhow::anyhow!("Worker has exited"))?;
        let raw = child
            .raw_handle()
            .ok_or_else(|| anyhow::anyhow!("Worker handle unavailable"))?;
        let process = unsafe { BorrowedHandle::borrow_raw(raw) }.try_clone_to_owned()?;
        let eager_snapshot = permissions
            & (ACCOUNT | DATABASES | IMAGE | INIT_MEMORY | PRELOAD_IMAGE | INIT_SAVED)
            != 0;
        let (revision, image, has_database_keys, account) = if !eager_snapshot {
            (0, None, false, None)
        } else {
            match self.query.key_snapshot().await {
                Ok(lease) => {
                    let snapshot = lease.key_material();
                    let image = if permissions & PRELOAD_IMAGE != 0 {
                        let (aes, xor) = snapshot.image_material();
                        aes.map(|aes| ImageMaterial { aes, xor })
                    } else {
                        None
                    };
                    let account = if permissions & INIT_SAVED != 0 {
                        snapshot
                            .verified_account_key()
                            .map(AccountMaterial::new)
                            .transpose()?
                    } else {
                        None
                    };
                    (
                        snapshot.revision(),
                        image,
                        snapshot.has_database_keys(),
                        account,
                    )
                }
                Err(error) if error.downcast_ref() == Some(&crate::key_store::Error::Missing) => {
                    (0, None, false, None)
                }
                Err(error) => return Err(error),
            }
        };
        let token = crate::service::transport::random_secret()?;
        let parent = crate::service::client::current_process_identity()?;
        let id = digest(token.as_bytes());
        let mut grants = self
            .grants
            .lock()
            .map_err(|_| anyhow::anyhow!("Worker grants unavailable"))?;
        ensure!(
            !self.closing.load(Ordering::Acquire),
            "Worker key service is stopping"
        );
        ensure!(grants.len() < MAX_GRANTS, "Worker key capacity exhausted");
        ensure!(!grants.contains_key(&id), "Worker capability collision");
        let active = Arc::new(AtomicBool::new(true));
        grants.insert(
            id,
            Arc::new(tokio::sync::Mutex::new(Grant {
                active: Arc::clone(&active),
                process,
                pid,
                permissions,
                revision,
                completed: None,
            })),
        );
        Ok(Some((
            Access {
                runtime: self.runtime.clone(),
                parent,
                capability: Secret::new(token.to_string()),
                revision,
                image,
                initialization: (permissions & (INIT_MEMORY | INIT_SAVED) != 0).then_some(
                    InitSeed {
                        has_database_keys,
                        account,
                    },
                ),
            },
            Registration {
                broker: Arc::clone(self),
                id,
                active,
            },
        )))
    }

    pub async fn read_databases(
        &self,
        peer_pid: u32,
        request: DatabaseReadRequest,
    ) -> std::result::Result<DatabaseSnapshot, ServiceError> {
        if self.closing.load(Ordering::Acquire) {
            return Err(ServiceError::unauthorized());
        }
        let grant = self
            .grants
            .lock()
            .map_err(|_| ServiceError::unauthorized())?
            .get(&digest(request.capability.as_str().as_bytes()))
            .cloned()
            .ok_or_else(ServiceError::unauthorized)?;
        let grant = grant.lock().await;
        if self.closing.load(Ordering::Acquire)
            || !grant.belongs_to_live_process(peer_pid)
            || grant.permissions & READ_DATABASES == 0
        {
            return Err(ServiceError::unauthorized());
        }
        let snapshot = match self.query.key_snapshot().await {
            Ok(lease) => DatabaseSnapshot {
                revision: lease.key_material().revision(),
                databases: Some(DatabaseKeys(lease.key_material().database_keys())),
            },
            Err(error) if error.downcast_ref() == Some(&crate::key_store::Error::Missing) => {
                DatabaseSnapshot {
                    revision: 0,
                    databases: None,
                }
            }
            Err(_) => {
                return Err(ServiceError::new(
                    "key_read_failed",
                    "Worker database snapshot unavailable",
                ))
            }
        };
        if self.closing.load(Ordering::Acquire) || !grant.belongs_to_live_process(peer_pid) {
            return Err(ServiceError::unauthorized());
        }
        if grant.revision != 0 && snapshot.revision != grant.revision {
            return Err(ServiceError::new(
                "conflict",
                "Worker database snapshot changed",
            ));
        }
        Ok(snapshot)
    }

    pub async fn read_image(
        &self,
        peer_pid: u32,
        request: ImageReadRequest,
    ) -> std::result::Result<ImageSnapshot, ServiceError> {
        if self.closing.load(Ordering::Acquire) {
            return Err(ServiceError::unauthorized());
        }
        let grant = self
            .grants
            .lock()
            .map_err(|_| ServiceError::unauthorized())?
            .get(&digest(request.capability.as_str().as_bytes()))
            .cloned()
            .ok_or_else(ServiceError::unauthorized)?;
        let mut grant = grant.lock().await;
        if self.closing.load(Ordering::Acquire)
            || !grant.belongs_to_live_process(peer_pid)
            || grant.permissions & READ_IMAGE == 0
        {
            return Err(ServiceError::unauthorized());
        }
        let snapshot = match self.query.key_snapshot().await {
            Ok(lease) => {
                let keys = lease.key_material();
                let (aes, xor) = keys.image_material();
                ImageSnapshot {
                    revision: keys.revision(),
                    material: aes.map(|aes| ImageMaterial { aes, xor }),
                }
            }
            Err(error) if error.downcast_ref() == Some(&crate::key_store::Error::Missing) => {
                ImageSnapshot {
                    revision: 0,
                    material: None,
                }
            }
            Err(_) => {
                return Err(ServiceError::new(
                    "key_read_failed",
                    "Worker image snapshot unavailable",
                ))
            }
        };
        if self.closing.load(Ordering::Acquire) || !grant.belongs_to_live_process(peer_pid) {
            return Err(ServiceError::unauthorized());
        }
        if grant.revision != 0 && snapshot.revision != grant.revision {
            return Err(ServiceError::new(
                "conflict",
                "Worker image snapshot changed",
            ));
        }
        grant.revision = snapshot.revision;
        Ok(snapshot)
    }

    pub async fn verify_image_revision(
        &self,
        peer_pid: u32,
        request: RevisionRequest,
    ) -> std::result::Result<(), ServiceError> {
        let grant = self
            .grants
            .lock()
            .map_err(|_| ServiceError::unauthorized())?
            .get(&digest(request.capability.as_str().as_bytes()))
            .cloned()
            .ok_or_else(ServiceError::unauthorized)?;
        let grant = grant.lock().await;
        if self.closing.load(Ordering::Acquire)
            || !grant.belongs_to_live_process(peer_pid)
            || grant.permissions & READ_IMAGE == 0
        {
            return Err(ServiceError::unauthorized());
        }
        let revision = match self.query.key_snapshot().await {
            Ok(lease) => lease.key_material().revision(),
            Err(error) if error.downcast_ref() == Some(&crate::key_store::Error::Missing) => 0,
            Err(_) => {
                return Err(ServiceError::new(
                    "key_read_failed",
                    "Key snapshot unavailable",
                ))
            }
        };
        if self.closing.load(Ordering::Acquire) || !grant.belongs_to_live_process(peer_pid) {
            return Err(ServiceError::unauthorized());
        }
        if request.expected_revision != grant.revision || revision != grant.revision {
            return Err(ServiceError::new(
                "conflict",
                "Worker image snapshot changed",
            ));
        }
        Ok(())
    }

    pub async fn update(
        self: &Arc<Self>,
        peer_pid: u32,
        mut request: UpdateRequest,
    ) -> std::result::Result<u64, ServiceError> {
        if self.closing.load(Ordering::Acquire) {
            return Err(ServiceError::unauthorized());
        }
        let grant = self
            .grants
            .lock()
            .map_err(|_| ServiceError::unauthorized())?
            .get(&digest(request.capability.as_str().as_bytes()))
            .cloned()
            .ok_or_else(ServiceError::unauthorized)?;
        let state = Arc::clone(self);
        // Keep completion/idempotency bookkeeping alive when the RPC waiter disconnects.
        tokio::spawn(async move {
            let mut grant = grant.lock().await;
            if state.closing.load(Ordering::Acquire) || !grant.belongs_to_live_process(peer_pid) {
                return Err(ServiceError::unauthorized());
            }
            if request.changes.is_empty()
                || request
                    .changes
                    .iter()
                    .any(|change| grant.permissions & change_permission(change) == 0)
            {
                return Err(ServiceError::unauthorized());
            }
            let signature = update_signature(&request);
            if let Some((previous, revision)) = grant.completed {
                if previous == signature {
                    return Ok(revision);
                }
            }
            if request.expected_revision != grant.revision {
                return Err(ServiceError::new("conflict", "Worker key revision changed"));
            }
            let changes = request
                .changes
                .iter_mut()
                .map(|change| {
                    let verified = crate::key_store::Verification::Verified;
                    match change {
                        MaterialChange::Account(key) => {
                            KeyChange::Account(std::mem::take(key), verified)
                        }
                        MaterialChange::Databases(keys) => {
                            KeyChange::Databases(std::mem::take(keys), verified)
                        }
                        MaterialChange::Image { aes, xor } => {
                            KeyChange::Image(*aes, *xor, verified)
                        }
                    }
                })
                .collect();
            let revision = state
                .query
                .update_keys(Some(request.expected_revision), changes)
                .await
                .map_err(update_error)?;
            grant.revision = revision;
            grant.completed = Some((signature, revision));
            Ok(revision)
        })
        .await
        .map_err(|_| ServiceError::new("key_update_failed", "Worker key update did not complete"))?
    }

    pub fn close(&self) {
        self.closing.store(true, Ordering::Release);
        if let Ok(mut grants) = self.grants.lock() {
            grants.clear();
        }
    }
}
