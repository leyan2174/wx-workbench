//! Production thin stdio adapter and authenticated transport, isolated from unrelated operations.
pub use mcp_voice_host::{asr, audio, crypto, daemon, db_cache, fixture_runtime,
    ipc, legacy, mcp, mcp_service, mcp_voice, protocol, runtime, toolkit_asr};
#[path = "../../../src/cli/mcp.rs"]
pub mod cli_mcp;
#[path = "../../../src/cli/mcp_tasks.rs"]
pub mod mcp_tasks;
#[path = "../../../src/config.rs"]
pub mod config;
#[path = "../../../src/attachment/local_files.rs"]
#[allow(dead_code)] // This slice uses publication guards but omits other media scan/proof APIs.
pub mod local_files;
pub mod attachment {
    pub use crate::local_files;
}
#[path = "../../../src/toolkit/private_file.rs"]
#[allow(dead_code)] // The CLI transport fixture does not exercise ACL inspection.
pub mod private_file;
pub mod toolkit {
    pub use crate::{audio, legacy, private_file, toolkit_asr as asr};
}
pub use service::query_client as transport;
#[allow(dead_code)]
pub mod service {
    pub mod query_client { include!(concat!(env!("OUT_DIR"), "/service_query_client.rs")); }
    pub mod config_pin { include!(concat!(env!("OUT_DIR"), "/service_config_pin.rs")); }
    pub mod plan { include!(concat!(env!("OUT_DIR"), "/service_plan.rs")); }
    pub mod settings { include!(concat!(env!("OUT_DIR"), "/service_settings.rs")); }
    pub use mcp_voice_host::service::{mcp, operation_requests};
    pub mod protocol { include!(concat!(env!("OUT_DIR"), "/service_protocol.rs")); }
    pub mod client { include!(concat!(env!("OUT_DIR"), "/service_client.rs")); }
    pub mod transport { include!(concat!(env!("OUT_DIR"), "/service_transport.rs")); }
}
pub mod cli {
    pub use crate::{asr, cli_mcp as mcp, mcp_voice, transport};
}
#[path = "../mcp-auth/mock.rs"]
pub mod authenticated_mock;

#[path = "../../support/mcp_argument_parsers.rs"]
pub mod operation_args;
