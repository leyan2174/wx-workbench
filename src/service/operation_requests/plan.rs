#[derive(Debug, Clone, Copy, Default)]
pub enum Mode {
    #[default]
    Blacklist,
    Whitelist,
}
