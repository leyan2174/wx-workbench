#[path = "../mcp-auth/build_support.rs"]
mod support;
fn main() {
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    support::authenticated_task_transport(&root, &out);
}
