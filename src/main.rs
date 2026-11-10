//! The `tay` command.

fn main() -> std::process::ExitCode {
    tay::cli::run(std::env::args_os().skip(1))
}
