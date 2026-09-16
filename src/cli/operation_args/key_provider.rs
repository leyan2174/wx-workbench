#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum KeyProvider {
    Saved,
    Memory,
    Account,
}

impl From<KeyProvider> for crate::service::operation_requests::key_provider::KeyProvider {
    fn from(value: KeyProvider) -> Self {
        match value {
            KeyProvider::Saved => Self::Saved,
            KeyProvider::Memory => Self::Memory,
            KeyProvider::Account => Self::Account,
        }
    }
}

impl From<crate::service::operation_requests::key_provider::KeyProvider> for KeyProvider {
    fn from(value: crate::service::operation_requests::key_provider::KeyProvider) -> Self {
        match value {
            crate::service::operation_requests::key_provider::KeyProvider::Saved => Self::Saved,
            crate::service::operation_requests::key_provider::KeyProvider::Memory => Self::Memory,
            crate::service::operation_requests::key_provider::KeyProvider::Account => Self::Account,
        }
    }
}
