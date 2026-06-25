//! Runtime eval CLI.

use datacenter_agent::runtime::eval::runner::{run, EvalMode};

fn main() {
    let pipeline_only = std::env::args().any(|arg| arg == "--pipeline-only");
    if !pipeline_only {
        eprintln!("usage: eval --pipeline-only");
        std::process::exit(2);
    }

    match run(EvalMode::PipelineOnly) {
        Ok(report) => {
            println!(
                "pipeline eval completed: passed={}, failed={}",
                report.passed, report.failed
            );
        }
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}
