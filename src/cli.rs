//! Fixed CLI surface parsed with `lexopt`.
//!
//! Accepted options are exactly `--help`, `--version`, `--once`, `--json`,
//! `--json-stream`, `--tiny`, `--gpu`, `--theme`, `--mode`, and `--no-color`.
//! Contains no telemetry or product-state rules.

use std::ffi::OsString;

use crate::config::{ModePreference, Theme};
use crate::model::PciBdf;

/// Authoritative CLI reference printed by `--help`.
pub(crate) const HELP: &str = "\
gpuflo — read-only AMD GPU instrument for Linux amdgpu hosts

Usage: gpuflo [OPTIONS]

Without options gpuflo runs the interactive terminal instrument.

Output modes (mutually exclusive):
  --once           Print one human-readable line per physical GPU, then exit
  --json           Print one pretty JSON snapshot of every physical GPU, then exit
  --json-stream    Print compact NDJSON snapshots continuously
  --tiny           Print one status line for the selected physical GPU, then exit

Selection:
  --gpu <SEL>      Select a physical GPU by display index, stable id, or PCI BDF.
                   Applies to --tiny and the initial interactive selection; the
                   all-GPU outputs (--once, --json, --json-stream) are unfiltered.

Visual options (interactive TUI only; stdout output is always ANSI-free):
  --theme <NAME>   buffalo | nord | monochrome
  --mode <NAME>    auto | mode | compact | mini | tiny
  --no-color       Disable color (a non-empty NO_COLOR does the same)

Other:
  --help           Print this help
  --version        Print the version

Configuration: $XDG_CONFIG_HOME/gpuflo/config.toml (fallback
~/.config/gpuflo/config.toml) may set theme, mode, and no_color only.
CLI flags override the file. Exit codes: 0 success (including partial
telemetry and broken pipe), 1 fatal runtime failure, 2 usage or
configuration error, 130 interrupted.
";

/// Which surface the invocation selects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputMode {
    /// Interactive TUI (default).
    Interactive,
    /// One human line per physical GPU.
    Once,
    /// One pretty all-GPU JSON snapshot.
    Json,
    /// Compact NDJSON at production cadence.
    JsonStream,
    /// One selected-GPU human status line.
    Tiny,
}

/// `--gpu` selector: display index, stable id, or PCI BDF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GpuSelector {
    Index(u32),
    Bdf(PciBdf),
    Id(String),
}

impl GpuSelector {
    fn parse(text: &str) -> Result<Self, UsageError> {
        if text.is_empty() {
            return Err(UsageError(
                "--gpu requires an index, id, or PCI BDF".to_owned(),
            ));
        }
        if text.bytes().all(|b| b.is_ascii_digit()) {
            return text
                .parse::<u32>()
                .map(Self::Index)
                .map_err(|_| UsageError(format!("GPU index out of range: {text}")));
        }
        if let Ok(bdf) = PciBdf::parse(text) {
            return Ok(Self::Bdf(bdf));
        }
        Ok(Self::Id(text.to_owned()))
    }
}

/// Parsed command line before configuration merging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CliOptions {
    pub output: OutputMode,
    pub gpu: Option<GpuSelector>,
    pub theme: Option<Theme>,
    pub mode: Option<ModePreference>,
    pub no_color: bool,
}

/// Complete parse result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Invocation {
    Help,
    Version,
    Run(CliOptions),
}

/// A command-line usage error; maps to exit 2.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub(crate) struct UsageError(pub String);

/// Parses the fixed flag surface from an argument iterator (without argv[0]).
pub(crate) fn parse<I>(args: I) -> Result<Invocation, UsageError>
where
    I: IntoIterator,
    I::Item: Into<OsString>,
{
    use lexopt::prelude::*;

    let mut parser = lexopt::Parser::from_args(args);
    let mut output: Option<OutputMode> = None;
    let mut options = CliOptions {
        output: OutputMode::Interactive,
        gpu: None,
        theme: None,
        mode: None,
        no_color: false,
    };

    let set_output = |mode: OutputMode, current: &mut Option<OutputMode>| {
        if let Some(existing) = current
            && *existing != mode
        {
            return Err(UsageError(
                "--once, --json, --json-stream, and --tiny are mutually exclusive".to_owned(),
            ));
        }
        *current = Some(mode);
        Ok(())
    };

    while let Some(arg) = parser.next().map_err(|e| UsageError(e.to_string()))? {
        match arg {
            Long("help") => return Ok(Invocation::Help),
            Long("version") => return Ok(Invocation::Version),
            Long("once") => set_output(OutputMode::Once, &mut output)?,
            Long("json") => set_output(OutputMode::Json, &mut output)?,
            Long("json-stream") => set_output(OutputMode::JsonStream, &mut output)?,
            Long("tiny") => set_output(OutputMode::Tiny, &mut output)?,
            Long("gpu") => {
                let value = parser
                    .value()
                    .map_err(|e| UsageError(e.to_string()))?
                    .into_string()
                    .map_err(|_| UsageError("--gpu value is not valid UTF-8".to_owned()))?;
                options.gpu = Some(GpuSelector::parse(&value)?);
            }
            Long("theme") => {
                let value = parser
                    .value()
                    .map_err(|e| UsageError(e.to_string()))?
                    .into_string()
                    .map_err(|_| UsageError("--theme value is not valid UTF-8".to_owned()))?;
                options.theme = Some(Theme::try_from(value).map_err(UsageError)?);
            }
            Long("mode") => {
                let value = parser
                    .value()
                    .map_err(|e| UsageError(e.to_string()))?
                    .into_string()
                    .map_err(|_| UsageError("--mode value is not valid UTF-8".to_owned()))?;
                options.mode = Some(ModePreference::try_from(value).map_err(UsageError)?);
            }
            Long("no-color") => options.no_color = true,
            other => {
                return Err(UsageError(format!(
                    "unexpected argument: {}",
                    render_arg(&other)
                )));
            }
        }
    }

    options.output = output.unwrap_or(OutputMode::Interactive);
    Ok(Invocation::Run(options))
}

fn render_arg(arg: &lexopt::Arg<'_>) -> String {
    use lexopt::Arg;
    match arg {
        Arg::Short(c) => format!("-{c}"),
        Arg::Long(name) => format!("--{name}"),
        Arg::Value(value) => value.to_string_lossy().into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(args: &[&str]) -> Result<Invocation, UsageError> {
        parse(args.iter().map(OsString::from))
    }

    fn options(args: &[&str]) -> CliOptions {
        match run(args).unwrap() {
            Invocation::Run(options) => options,
            other => panic!("expected run invocation, got {other:?}"),
        }
    }

    #[test]
    fn default_is_interactive() {
        let options = options(&[]);
        assert_eq!(options.output, OutputMode::Interactive);
        assert_eq!(options.gpu, None);
        assert!(!options.no_color);
    }

    #[test]
    fn output_modes_parse_and_exclude_each_other() {
        assert_eq!(options(&["--once"]).output, OutputMode::Once);
        assert_eq!(options(&["--json"]).output, OutputMode::Json);
        assert_eq!(options(&["--json-stream"]).output, OutputMode::JsonStream);
        assert_eq!(options(&["--tiny"]).output, OutputMode::Tiny);
        for pair in [
            ["--once", "--json"],
            ["--json", "--json-stream"],
            ["--tiny", "--once"],
            ["--json-stream", "--tiny"],
        ] {
            assert!(run(&pair).is_err(), "{pair:?} should be rejected");
        }
    }

    #[test]
    fn gpu_selector_forms() {
        assert_eq!(options(&["--gpu", "0"]).gpu, Some(GpuSelector::Index(0)));
        assert_eq!(
            options(&["--gpu", "0000:41:00.0"]).gpu,
            Some(GpuSelector::Bdf(PciBdf::parse("0000:41:00.0").unwrap()))
        );
        assert_eq!(
            options(&["--gpu", "gpu-73fbc1"]).gpu,
            Some(GpuSelector::Id("gpu-73fbc1".to_owned()))
        );
        assert!(run(&["--gpu", ""]).is_err());
        assert!(run(&["--gpu"]).is_err());
    }

    #[test]
    fn visual_flags_parse_and_reject_unknown_values() {
        let options = options(&["--theme", "nord", "--mode", "compact", "--no-color"]);
        assert_eq!(options.theme, Some(Theme::Nord));
        assert_eq!(options.mode, Some(ModePreference::Compact));
        assert!(options.no_color);
        assert!(run(&["--theme", "solarized"]).is_err());
        assert!(run(&["--mode", "full"]).is_err());
    }

    #[test]
    fn unknown_flags_and_positionals_are_usage_errors() {
        assert!(run(&["--frobnicate"]).is_err());
        assert!(run(&["-x"]).is_err());
        assert!(run(&["positional"]).is_err());
    }

    #[test]
    fn help_and_version_win() {
        assert_eq!(run(&["--help"]).unwrap(), Invocation::Help);
        assert_eq!(run(&["--version"]).unwrap(), Invocation::Version);
        assert_eq!(run(&["--json", "--help"]).unwrap(), Invocation::Help);
    }
}
