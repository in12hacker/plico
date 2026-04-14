//! Plico — AI-Native Operating System
//!
//! Entry point. Run `plicod` (daemon) or `aicli` (CLI) binaries.

fn main() {
    eprintln!("Run 'cargo run --bin plicod' for the daemon");
    eprintln!("Run 'cargo run --bin aicli -- <command>' for the CLI");
    eprintln!("See --help for each binary.");
}
