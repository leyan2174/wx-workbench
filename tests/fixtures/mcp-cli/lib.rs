//! Production thin stdio adapter and authenticated transport, isolated from unrelated operations.
pub use mcp_host::{
    config, crypto, daemon, db_cache, fixture_runtime, infrastructure, ipc, mcp, mcp_service,
    protocol, runtime,
};
#[path = "../../../src/cli/mcp.rs"]
pub mod cli_mcp;
#[path = "../../../src/attachment/local_files.rs"]
#[allow(dead_code)] // This slice uses publication guards but omits other media scan/proof APIs.
pub mod local_files;
#[path = "../../../src/cli/mcp_tasks.rs"]
pub mod mcp_tasks;
mod tasks {
    include!(concat!(env!("OUT_DIR"), "/cli_task_validation.rs"));
}
pub mod attachment {
    pub use crate::local_files;
}
#[path = "../../../src/private_file.rs"]
#[allow(dead_code)] // The CLI transport fixture does not exercise ACL inspection.
pub mod private_file;
pub use service::query_client as transport;
#[allow(dead_code)]
pub mod service {
    pub mod history_export {
        include!(concat!(env!("OUT_DIR"), "/service_history_export.rs"));
    }
    pub mod time {
        include!(concat!(env!("OUT_DIR"), "/service_time.rs"));
    }
    pub mod task_artifacts {
        include!(concat!(env!("OUT_DIR"), "/service_task_artifacts.rs"));
    }
    pub mod query_client {
        include!(concat!(env!("OUT_DIR"), "/service_query_client.rs"));
    }
    pub use mcp_host::config_pin;
    pub mod plan {
        include!(concat!(env!("OUT_DIR"), "/service_plan.rs"));
    }
    pub mod settings {
        include!(concat!(env!("OUT_DIR"), "/service_settings.rs"));
    }
    pub use mcp_host::service::mcp;
    pub mod protocol {
        include!(concat!(env!("OUT_DIR"), "/service_protocol.rs"));
    }
    pub mod worker_keys {
        include!(concat!(env!("OUT_DIR"), "/service_worker_keys.rs"));
    }
    pub mod client {
        include!(concat!(env!("OUT_DIR"), "/service_client.rs"));
    }
    pub mod transport {
        include!(concat!(env!("OUT_DIR"), "/service_transport.rs"));
    }
}
pub mod cli {
    pub use crate::{cli_mcp as mcp, transport};
}
#[path = "../mcp-auth/mock.rs"]
pub mod authenticated_mock;
