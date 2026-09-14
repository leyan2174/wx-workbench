#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Args {
    /// Explicitly retain account-bound material lacking local verification evidence.
    pub allow_unverified: bool,
    /// Remove legacy image fields after verified publication; shared files are retained.
    pub cleanup_legacy: bool,
}
