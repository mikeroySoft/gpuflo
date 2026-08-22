//! The gpuflo binary: process exit mapping only.

fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(gpuflo::run_from_env())
}
