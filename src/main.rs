//! tay: Topology Automation via YAML.
//!
//! This release only takes the name. Nothing here runs a stack yet, so the
//! binary says so and exits with a failure code: a tool whose purpose is to
//! tell the truth about a stack must not report success for doing nothing.

fn main() {
    eprintln!(
        "tay {}: Topology Automation via YAML.",
        env!("CARGO_PKG_VERSION")
    );
    eprintln!(
        "This is the placeholder era: the name is taken and the work has begun, but there is nothing to run yet."
    );
    std::process::exit(2);
}
