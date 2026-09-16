//! Account-bound, versioned key snapshots. No plaintext fallback or implicit migration.
pub(crate) mod dpapi;

use crate::infrastructure::configuration::{ConfigLock, Snapshot as FileSnapshot};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fmt,
    path::{Path, PathBuf},
};
use zeroize::{Zeroize, Zeroizing};

const MAGIC: &[u8] = b"WXKEYS\0\x01";
const MAX_KEYS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Missing,
    LegacyMigrationRequired,
    Invalid,
    WrongAccount,
    Protection,
    Conflict,
    Busy,
    Io,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Missing => {
                "Encrypted key store is missing; explicitly initialize keys for the selected account"
            }
            Self::LegacyMigrationRequired => {
                "Legacy key material is unsupported; configure a current DPAPI key store and explicitly initialize the selected account; plaintext fallback is disabled"
            }
            Self::Invalid => "Invalid key store format, version or key material",
            Self::WrongAccount => "Key store belongs to a different account",
            Self::Protection => "Current-user DPAPI protection failed; no plaintext fallback",
            Self::Conflict => {
                "Key store update conflicts with the current revision or existing material"
            }
            Self::Busy => "Account key store is being updated; retry after the current update",
            Self::Io => "Key store file or path validation failed",
        })
    }
}
impl std::error::Error for Error {}
type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verification {
    Verified,
    Unverified,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Material {
    bytes: Vec<u8>,
    verification: Verification,
}
impl Drop for Material {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    account: String,
    revision: u64,
    account_key: Option<Material>,
    #[serde(deserialize_with = "unique_database_keys")]
    database_keys: BTreeMap<String, Material>,
    // When present, AES bytes followed by XOR (17 bytes).
    image_key: Option<Material>,
}

pub struct Snapshot {
    record: Record,
}

fn unique_database_keys<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, Material>, D::Error> {
    struct Unique;
    impl<'de> serde::de::Visitor<'de> for Unique {
        type Value = BTreeMap<String, Material>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("unique database records")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut input: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut keys = BTreeMap::new();
            while let Some((name, material)) = input.next_entry::<String, Material>()? {
                if keys.len() >= MAX_KEYS || keys.insert(name, material).is_some() {
                    return Err(serde::de::Error::custom(
                        "duplicate or excessive database records",
                    ));
                }
            }
            Ok(keys)
        }
    }
    deserializer.deserialize_map(Unique)
}
impl fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeySnapshot")
            .field("revision", &self.revision())
            .field("materials", &"[REDACTED]")
            .finish()
    }
}
impl Snapshot {
    pub fn revision(&self) -> u64 {
        self.record.revision
    }
    #[cfg(test)]
    pub fn account_key(&self) -> Option<&[u8]> {
        self.record
            .account_key
            .as_ref()
            .map(|key| key.bytes.as_slice())
    }
    pub fn verified_account_key(&self) -> Option<&[u8]> {
        self.record
            .account_key
            .as_ref()
            .filter(|key| key.verification == Verification::Verified)
            .map(|key| key.bytes.as_slice())
    }
    pub fn image_key(&self) -> Option<([u8; 16], u8)> {
        self.record.image_key.as_ref().map(|key| {
            (
                key.bytes[..16].try_into().expect("validated AES length"),
                key.bytes[16],
            )
        })
    }
    pub fn image_material(&self) -> (Option<[u8; 16]>, u8) {
        self.image_key()
            .map_or((None, 0x88), |(aes, xor)| (Some(aes), xor))
    }
    pub fn database_keys(&self) -> HashMap<String, String> {
        self.record
            .database_keys
            .iter()
            .map(|(name, key)| {
                (
                    name.clone(),
                    key.bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
                )
            })
            .collect()
    }

    pub fn has_database_keys(&self) -> bool {
        !self.record.database_keys.is_empty()
    }
}

pub enum Update<'a> {
    Account(&'a [u8], Verification),
    Databases(&'a HashMap<String, String>, Verification),
    Image(&'a [u8; 16], u8, Verification),
}

pub struct Store {
    path: PathBuf,
    binding: String,
    protected: Vec<PathBuf>,
}

pub(crate) fn normalized(path: &Path) -> Result<String> {
    let path = std::path::absolute(path).map_err(|_| Error::Io)?;
    Ok(path
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .replace('/', "\\")
        .to_lowercase())
}

pub(crate) fn database_name(name: &str) -> Result<String> {
    let name = name.replace('\\', "/");
    if name.is_empty()
        || name.len() > 1024
        || !name.to_ascii_lowercase().ends_with(".db")
        || name.contains([':', '*', '?', '<', '>', '|', '"'])
        || name.chars().any(char::is_control)
        || name.split('/').any(|part| {
            let stem = part.split('.').next().unwrap_or("").to_ascii_lowercase();
            part.is_empty()
                || part == "."
                || part == ".."
                || part.ends_with(['.', ' '])
                || matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
                || ((stem.starts_with("com") || stem.starts_with("lpt"))
                    && stem.len() == 4
                    && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        })
    {
        return Err(Error::Invalid);
    }
    Ok(name)
}

impl Store {
    pub fn for_config(config: &crate::config::Config) -> Result<Self> {
        let path = config
            .key_store
            .as_ref()
            .ok_or(Error::LegacyMigrationRequired)?;
        Self::new(
            &config.db_dir,
            &config.keys_file,
            path,
            vec![
                config.db_dir.clone(),
                config.keys_file.clone(),
                config.decrypted_dir.clone(),
            ],
        )
    }

    pub fn for_runtime(runtime: &crate::runtime::RuntimeContext) -> Result<Self> {
        let mut store = Self::for_config(&runtime.config)?;
        store
            .protected
            .extend([runtime.config_path.clone(), runtime.directory.clone()]);
        crate::infrastructure::publication::check_target(&store.path, &store.protected)
            .map_err(|_| Error::Io)?;
        Ok(store)
    }

    pub fn new(
        db_dir: &Path,
        identity_anchor: &Path,
        path: &Path,
        protected: Vec<PathBuf>,
    ) -> Result<Self> {
        let database = db_dir.canonicalize().map_err(|_| Error::Io)?;
        let binding = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(normalized(&database)?, normalized(identity_anchor)?))
                    .map_err(|_| Error::Invalid)?
            )
        );
        let path = std::path::absolute(path).map_err(|_| Error::Io)?;
        crate::infrastructure::publication::check_target(&path, &protected)
            .map_err(|_| Error::Io)?;
        Ok(Self {
            path,
            binding,
            protected,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn decode(&self, file: &FileSnapshot) -> Result<Snapshot> {
        let bytes = file.bytes().ok_or(Error::Missing)?;
        if !bytes.starts_with(MAGIC) {
            return Err(if bytes.first().is_some_and(|b| matches!(b, b'{' | b'[')) {
                Error::LegacyMigrationRequired
            } else {
                Error::Invalid
            });
        }
        let plain = dpapi::transform(&bytes[MAGIC.len()..], true).map_err(|_| Error::Protection)?;
        let record: Record = serde_json::from_slice(&plain).map_err(|_| Error::Invalid)?;
        if record.version != 1 || record.revision == 0 || record.database_keys.len() > MAX_KEYS {
            return Err(Error::Invalid);
        }
        if record.account != self.binding {
            return Err(Error::WrongAccount);
        }
        if record
            .account_key
            .as_ref()
            .is_some_and(|key| key.bytes.len() != 32)
            || record
                .image_key
                .as_ref()
                .is_some_and(|key| key.bytes.len() != 17)
        {
            return Err(Error::Invalid);
        }
        let mut names = HashSet::new();
        for (name, key) in &record.database_keys {
            if database_name(name)? != *name
                || !names.insert(name.to_lowercase())
                || key.bytes.len() != 32
            {
                return Err(Error::Invalid);
            }
        }
        Ok(Snapshot { record })
    }

    pub fn load(&self) -> Result<Snapshot> {
        let file = FileSnapshot::capture(&self.path).map_err(|_| Error::Io)?;
        self.decode(&file)
    }

    pub fn update(&self, expected: Option<u64>, updates: &[Update<'_>]) -> Result<Snapshot> {
        if updates.is_empty() {
            return Err(Error::Invalid);
        }
        let _lock = ConfigLock::acquire(&self.path.with_extension("key-update.lock"))
            .map_err(|_| Error::Busy)?;
        let file = FileSnapshot::capture(&self.path).map_err(|_| Error::Io)?;
        let mut record = if file.existed() {
            self.decode(&file)?.record
        } else {
            Record {
                version: 1,
                account: self.binding.clone(),
                revision: 0,
                account_key: None,
                database_keys: BTreeMap::new(),
                image_key: None,
            }
        };
        if expected.is_some_and(|revision| revision != record.revision) {
            return Err(Error::Conflict);
        }
        let mut changed = false;
        if updates
            .iter()
            .any(|update| matches!(update, Update::Databases(..)))
        {
            let mut retained = HashSet::new();
            for update in updates {
                if let Update::Databases(keys, _) = update {
                    for name in keys.keys() {
                        retained.insert(database_name(name)?);
                    }
                }
            }
            let previous = record.database_keys.len();
            record
                .database_keys
                .retain(|name, _| retained.contains(name));
            changed |= previous != record.database_keys.len();
        }
        for update in updates {
            match update {
                Update::Account(bytes, verification) => {
                    if bytes.len() != 32 {
                        return Err(Error::Invalid);
                    }
                    changed |= assign(&mut record.account_key, bytes, *verification);
                }
                Update::Image(aes, xor, verification) => {
                    let mut bytes = Zeroizing::new(aes.to_vec());
                    bytes.push(*xor);
                    changed |= assign(&mut record.image_key, &bytes, *verification);
                }
                Update::Databases(keys, verification) => {
                    let mut names = HashSet::new();
                    for (name, encoded) in *keys {
                        let name = database_name(name)?;
                        if !names.insert(name.to_lowercase())
                            || encoded.len() != 64
                            || !encoded.bytes().all(|b| b.is_ascii_hexdigit())
                        {
                            return Err(Error::Invalid);
                        }
                        let bytes = Zeroizing::new(
                            (0..32)
                                .map(|i| {
                                    u8::from_str_radix(&encoded[i * 2..i * 2 + 2], 16)
                                        .map_err(|_| Error::Invalid)
                                })
                                .collect::<Result<Vec<_>>>()?,
                        );
                        if record.database_keys.keys().any(|existing| {
                            existing != &name && existing.eq_ignore_ascii_case(&name)
                        }) {
                            return Err(Error::Conflict);
                        }
                        let mut existing = record.database_keys.remove(&name);
                        changed |= assign(&mut existing, &bytes, *verification);
                        record
                            .database_keys
                            .insert(name, existing.expect("assigned material"));
                    }
                }
            }
        }
        if record.database_keys.len() > MAX_KEYS {
            return Err(Error::Invalid);
        }
        if !changed && file.existed() {
            return Ok(Snapshot { record });
        }
        record.revision = record.revision.checked_add(1).ok_or(Error::Invalid)?;
        let plain = Zeroizing::new(serde_json::to_vec(&record).map_err(|_| Error::Invalid)?);
        let encrypted = dpapi::transform(&plain, false).map_err(|_| Error::Protection)?;
        let mut bytes = Vec::with_capacity(MAGIC.len() + encrypted.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&encrypted);
        file.write_bytes(&bytes, &self.protected)
            .map_err(|_| Error::Io)?;
        let verified = self.load()?;
        if verified.revision() != record.revision {
            return Err(Error::Conflict);
        }
        Ok(verified)
    }
}

fn assign(target: &mut Option<Material>, bytes: &[u8], verification: Verification) -> bool {
    if let Some(previous) = target {
        if previous.bytes == bytes
            && (previous.verification == verification || verification == Verification::Unverified)
        {
            return false;
        }
    }
    *target = Some(Material {
        bytes: bytes.to_vec(),
        verification,
    });
    true
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) fn seed_databases(runtime: &crate::runtime::RuntimeContext, keys: serde_json::Value) {
    let keys: HashMap<String, String> = serde_json::from_value(keys).unwrap();
    Store::for_runtime(runtime)
        .unwrap()
        .update(None, &[Update::Databases(&keys, Verification::Verified)])
        .unwrap();
}
