//! Runtime eval CLI.

use std::path::PathBuf;

use clap::Parser;
use datacenter_agent::runtime::eval::runner::{run, EvalMode};

fn main() {
    let args = Args::parse();
    let mode = match args.mode() {
        Ok(mode) => mode,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };

    match run(mode) {
        Ok(report) => {
            println!(
                "eval completed: passed={}, failed={}, latency_ms={}, tokens={}, refusals={}, fallbacks={}",
                report.passed,
                report.failed,
                report.latency_ms,
                report.tokens,
                report.refusals,
                report.fallbacks
            );
            if let Some(provenance) = report.provenance {
                println!("provenance: {provenance}");
            }
            for regression in report.regressions {
                println!("regression: {regression}");
            }
        }
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}

#[derive(Debug, Parser)]
#[command(about = "Run datacenter agent runtime evals")]
struct Args {
    /// Run the offline deterministic input pipeline eval.
    #[arg(long)]
    pipeline_only: bool,
    /// Run response eval mode.
    #[arg(long)]
    response: bool,
    /// Replay response eval from a recorded artifact.
    #[arg(long, value_name = "ARTIFACT")]
    replay: Option<PathBuf>,
    /// Run live response eval against provider-backed behavior.
    #[arg(long)]
    live: bool,
    /// Approved response baseline used by live mode.
    #[arg(
        long,
        value_name = "BASELINE",
        default_value = "config/runtime/evals/response-baseline.json"
    )]
    baseline: PathBuf,
}

impl Args {
    fn mode(self) -> Result<EvalMode, String> {
        if self.pipeline_only {
            if self.response || self.replay.is_some() || self.live {
                return Err("--pipeline-only cannot be combined with response eval flags".into());
            }
            return Ok(EvalMode::PipelineOnly);
        }

        if !self.response {
            return Err("usage: eval --pipeline-only | --response --replay <artifact> | --response --live [--baseline <path>]".into());
        }

        match (self.replay, self.live) {
            (Some(artifact), false) => Ok(EvalMode::ResponseReplay { artifact }),
            (None, true) => Ok(EvalMode::ResponseLive {
                baseline: self.baseline,
            }),
            (Some(_), true) => Err("--replay and --live are mutually exclusive".into()),
            (None, false) => Err("--response requires --replay <artifact> or --live".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pipeline_only_mode() {
        let args = Args::try_parse_from(["eval", "--pipeline-only"]).expect("args should parse");

        assert_eq!(
            args.mode().expect("mode should build"),
            EvalMode::PipelineOnly
        );
    }

    #[test]
    fn parses_response_replay_mode() {
        let args = Args::try_parse_from(["eval", "--response", "--replay", "artifact.json"])
            .expect("args should parse");

        assert_eq!(
            args.mode().expect("mode should build"),
            EvalMode::ResponseReplay {
                artifact: PathBuf::from("artifact.json")
            }
        );
    }

    #[test]
    fn parses_response_live_mode() {
        let args =
            Args::try_parse_from(["eval", "--response", "--live"]).expect("args should parse");

        assert_eq!(
            args.mode().expect("mode should build"),
            EvalMode::ResponseLive {
                baseline: PathBuf::from("config/runtime/evals/response-baseline.json")
            }
        );
    }

    #[test]
    fn rejects_missing_response_submode() {
        let args = Args::try_parse_from(["eval", "--response"]).expect("args should parse");

        assert!(args
            .mode()
            .expect_err("mode should fail")
            .contains("--response requires"));
    }
}
