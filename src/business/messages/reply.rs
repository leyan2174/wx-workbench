//! A quoted-message preview, independent of transport and storage formats.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub text: String,
    /// Display-only label (including "me"), not an account-scoped author identity.
    pub sender_label: String,
    pub summary: String,
}
