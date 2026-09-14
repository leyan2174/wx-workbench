#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
pub enum Mode {
    #[default]
    Blacklist,
    Whitelist,
}

impl From<Mode> for crate::service::operation_requests::plan::Mode {
    fn from(value: Mode) -> Self {
        match value {
            Mode::Blacklist => Self::Blacklist,
            Mode::Whitelist => Self::Whitelist,
        }
    }
}

impl From<crate::service::operation_requests::plan::Mode> for Mode {
    fn from(value: crate::service::operation_requests::plan::Mode) -> Self {
        match value {
            crate::service::operation_requests::plan::Mode::Blacklist => Self::Blacklist,
            crate::service::operation_requests::plan::Mode::Whitelist => Self::Whitelist,
        }
    }
}
