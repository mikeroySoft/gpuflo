//! The gruflo binary: process exit mapping only.

fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(gruflo::run_from_env())
}
