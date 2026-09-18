pub use super::operation_args::image_keys::*;
// Argument adaptation and authenticated daemon operation forwarding only.
use crate::service::operations::Operation;
use anyhow::{ensure, Result};
use std::io::IsTerminal;

#[derive(Debug, clap::Args)]
pub struct ImportArgs {
    /// Read one bounded material JSON object from a private pipe, never the terminal.
    #[arg(long, required = true)]
    stdin: bool,
    /// Host-selected offline DAT sample root; defaults to the attached account's samples.
    #[arg(long)]
    sample_root: Option<std::path::PathBuf>,
    /// Verify the imported material without saving it.
    #[arg(long)]
    no_save: bool,
    /// Local sample verification budget in seconds; does not authorize memory scanning.
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u64).range(1..=3600))]
    timeout: u64,
    /// Maximum local sample bytes to read, in MiB.
    #[arg(long, default_value_t = 4096, value_parser = clap::value_parser!(u32).range(1..=32768))]
    max_mib: u32,
}

pub fn cmd_import(args: ImportArgs) -> Result<()> {
    ensure!(args.stdin, "Image material import requires private stdin");
    let input = std::io::stdin();
    ensure!(
        !input.is_terminal(),
        "Image material import requires non-interactive stdin"
    );
    let runtime = crate::runtime::RuntimeContext::load()?;
    let sealed = crate::service::image_import::seal_stdin(
        &runtime,
        input.lock(),
        crate::service::image_import::ImportOptions {
            sample_root: args.sample_root,
            no_save: args.no_save,
            timeout: args.timeout,
            max_mib: args.max_mib as usize,
        },
    )?;
    crate::service::operation_client::run_for(
        &runtime,
        Operation::ImportImageMaterial { args: sealed },
    )
}

pub fn cmd(args: Args) -> Result<()> {
    crate::service::operation_client::run(Operation::ImageKeys { args: args.into() })
}

pub fn cmd_monitor(args: MonitorArgs) -> Result<()> {
    crate::service::operation_client::run(Operation::ImageKeyMonitor { args: args.into() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Invocation {
        #[command(flatten)]
        args: ImportArgs,
    }

    #[test]
    fn import_stdin_is_explicit_with_bounded_verification_options() {
        assert!(Invocation::try_parse_from(["import-image"]).is_err());
        let args = Invocation::try_parse_from(["import-image", "--stdin"])
            .unwrap()
            .args;
        assert!(args.stdin);
        assert!(args.sample_root.is_none());
        assert!(!args.no_save);
        assert_eq!(args.timeout, 120);
        assert_eq!(args.max_mib, 4096);
        let args = Invocation::try_parse_from([
            "import-image",
            "--stdin",
            "--no-save",
            "--sample-root",
            "C:/offline/DAT samples",
            "--timeout",
            "3600",
            "--max-mib",
            "32768",
        ])
        .unwrap()
        .args;
        assert!(args.no_save);
        assert_eq!(
            args.sample_root,
            Some(std::path::PathBuf::from("C:/offline/DAT samples"))
        );
        assert_eq!(args.timeout, 3600);
        assert_eq!(args.max_mib, 32768);
        for (flag, value) in [
            ("--timeout", "0"),
            ("--timeout", "3601"),
            ("--max-mib", "0"),
            ("--max-mib", "32769"),
            ("--aes-key", "synthetic"),
            ("--image-key-file", "synthetic.json"),
            ("--input", "synthetic.json"),
            ("--envelope", "synthetic"),
            ("--xor-key", "136"),
        ] {
            assert!(
                Invocation::try_parse_from(["import-image", "--stdin", flag, value]).is_err(),
                "{flag}"
            );
        }
    }
}
