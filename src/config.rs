//! Sparse presentation configuration and path resolution.
//!
//! Owns built-in defaults and the one precedence merge:
//! built-in defaults → optional TOML overrides → CLI flags, with any
//! non-empty `NO_COLOR` as an unconditional final color disable. Sources,
//! output, and UI receive typed resolved options and never reread the
//! environment or TOML.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::cli::CliOptions;

/// Explicit environment inputs; the only place process env is read.
#[derive(Debug, Clone, Default)]
pub(crate) struct Environment {
    /// `$XDG_CONFIG_HOME`.
    pub xdg_config_home: Option<PathBuf>,
    /// `$XDG_STATE_HOME`.
    pub xdg_state_home: Option<PathBuf>,
    /// `$HOME`.
    pub home: Option<PathBuf>,
    /// `$NO_COLOR`; any non-empty value disables color.
    pub no_color: Option<std::ffi::OsString>,
    /// Whether terminal environment evidence advertises truecolor.
    pub truecolor: bool,
}

impl Environment {
    /// Captures the relevant variables from the process environment.
    pub fn from_process() -> Self {
        fn path(name: &str) -> Option<PathBuf> {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        }
        Self {
            xdg_config_home: path("XDG_CONFIG_HOME"),
            xdg_state_home: path("XDG_STATE_HOME"),
            home: path("HOME"),
            no_color: std::env::var_os("NO_COLOR"),
            truecolor: Self::terminal_truecolor(),
        }
    }

    fn terminal_truecolor() -> bool {
        ["COLORTERM", "TERM"]
            .into_iter()
            .filter_map(std::env::var_os)
            .map(|value| value.to_string_lossy().to_ascii_lowercase())
            .any(|value| {
                value.contains("truecolor") || value.contains("24bit") || value.contains("direct")
            })
    }
    /// `$XDG_CONFIG_HOME/gpuflo/config.toml`, falling back to
    /// `~/.config/gpuflo/config.toml`. `None` when neither root resolves.
    pub fn config_path(&self) -> Option<PathBuf> {
        if let Some(xdg) = &self.xdg_config_home {
            return Some(xdg.join("gpuflo/config.toml"));
        }
        self.home
            .as_ref()
            .map(|home| home.join(".config/gpuflo/config.toml"))
    }

    /// `$XDG_STATE_HOME/gpuflo/daily.json`, falling back to
    /// `~/.local/state/gpuflo/daily.json`. `None` disables persistence.
    pub fn summary_path(&self) -> Option<PathBuf> {
        if let Some(xdg) = &self.xdg_state_home {
            return Some(xdg.join("gpuflo/daily.json"));
        }
        self.home
            .as_ref()
            .map(|home| home.join(".local/state/gpuflo/daily.json"))
    }

    /// Whether `NO_COLOR` unconditionally disables color.
    pub fn no_color_set(&self) -> bool {
        self.no_color.as_ref().is_some_and(|v| !v.is_empty())
    }
}

/// Built-in semantic palette selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub(crate) enum Theme {
    /// Approved default: warm cream, rust, amber, restrained fault red.
    Buffalo,
    /// Restrained cool alternative with the same semantic roles.
    Nord,
    /// Neutral palette for minimal-color terminals.
    Monochrome,
}

impl Theme {
    pub fn name(self) -> &'static str {
        match self {
            Self::Buffalo => "buffalo",
            Self::Nord => "nord",
            Self::Monochrome => "monochrome",
        }
    }

    /// Cycles to the next built-in theme (session `t` key).
    pub fn next(self) -> Self {
        match self {
            Self::Buffalo => Self::Nord,
            Self::Nord => Self::Monochrome,
            Self::Monochrome => Self::Buffalo,
        }
    }
}

impl TryFrom<String> for Theme {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "buffalo" => Ok(Self::Buffalo),
            "nord" => Ok(Self::Nord),
            "monochrome" => Ok(Self::Monochrome),
            other => Err(format!(
                "unknown theme {other:?}; expected buffalo, nord, or monochrome"
            )),
        }
    }
}

/// Preferred responsive surface. `mode` names the full instrument cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub(crate) enum ModePreference {
    /// Richest approved surface that fits the current terminal.
    Auto,
    /// The full selected-GPU instrument cluster.
    Mode,
    Compact,
    Mini,
    Tiny,
}

impl ModePreference {
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Mode => "mode",
            Self::Compact => "compact",
            Self::Mini => "mini",
            Self::Tiny => "tiny",
        }
    }
}

impl TryFrom<String> for ModePreference {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "auto" => Ok(Self::Auto),
            "mode" => Ok(Self::Mode),
            "compact" => Ok(Self::Compact),
            "mini" => Ok(Self::Mini),
            "tiny" => Ok(Self::Tiny),
            other => Err(format!(
                "unknown mode {other:?}; expected auto, mode, compact, mini, or tiny"
            )),
        }
    }
}

/// The closed sparse TOML schema: exactly `theme`, `mode`, and `no_color`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    theme: Option<Theme>,
    mode: Option<ModePreference>,
    no_color: Option<bool>,
}

/// A startup configuration failure; maps to exit 2.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ConfigError {
    #[error("cannot read config file {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid config file {}: {message}", path.display())]
    Invalid { path: PathBuf, message: String },
}

/// Typed resolved presentation options for the interactive surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PresentationOptions {
    pub theme: Theme,
    pub mode_preference: ModePreference,
    pub color_enabled: bool,
    pub truecolor: bool,
}

impl Default for PresentationOptions {
    fn default() -> Self {
        Self {
            theme: Theme::Buffalo,
            mode_preference: ModePreference::Auto,
            color_enabled: true,
            truecolor: false,
        }
    }
}

/// Loads the optional sparse TOML file. Missing or blank files are valid;
/// malformed, wrong-type, and unknown-key files are startup errors.
fn load_file(path: &Path) -> Result<FileConfig, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FileConfig::default());
        }
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    toml::from_str(&text).map_err(|error| ConfigError::Invalid {
        path: path.to_owned(),
        message: error.to_string(),
    })
}

/// Resolves presentation options once at startup:
/// built-in defaults → TOML overrides → CLI overrides → `NO_COLOR`.
pub(crate) fn resolve(
    environment: &Environment,
    cli: &CliOptions,
) -> Result<PresentationOptions, ConfigError> {
    let file = match environment.config_path() {
        Some(path) => load_file(&path)?,
        None => FileConfig::default(),
    };
    let defaults = PresentationOptions::default();
    let theme = cli.theme.or(file.theme).unwrap_or(defaults.theme);
    let mode_preference = cli.mode.or(file.mode).unwrap_or(defaults.mode_preference);
    let disabled = environment.no_color_set() || cli.no_color || file.no_color.unwrap_or(false);
    Ok(PresentationOptions {
        theme,
        mode_preference,
        color_enabled: !disabled,
        truecolor: environment.truecolor,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{CliOptions, OutputMode};

    fn cli_defaults() -> CliOptions {
        CliOptions {
            output: OutputMode::Interactive,
            gpu: None,
            theme: None,
            mode: None,
            no_color: false,
        }
    }

    fn temp_config(content: &str) -> (tempdir::TempDir, Environment) {
        let dir = tempdir::TempDir::new();
        std::fs::create_dir_all(dir.path().join("gpuflo")).unwrap();
        std::fs::write(dir.path().join("gpuflo/config.toml"), content).unwrap();
        let env = Environment {
            xdg_config_home: Some(dir.path().to_owned()),
            ..Environment::default()
        };
        (dir, env)
    }

    /// Minimal unique temp dirs without a dev-dependency.
    mod tempdir {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU64, Ordering};

        static COUNTER: AtomicU64 = AtomicU64::new(0);

        pub struct TempDir(PathBuf);

        impl TempDir {
            pub fn new() -> Self {
                let path = std::env::temp_dir().join(format!(
                    "gpuflo-test-{}-{}",
                    std::process::id(),
                    COUNTER.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::create_dir_all(&path).unwrap();
                Self(path)
            }

            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    #[test]
    fn xdg_config_home_takes_precedence_over_home() {
        let env = Environment {
            xdg_config_home: Some(PathBuf::from("/xdg")),
            home: Some(PathBuf::from("/home/user")),
            ..Environment::default()
        };
        assert_eq!(
            env.config_path().unwrap(),
            PathBuf::from("/xdg/gpuflo/config.toml")
        );
        let env = Environment {
            home: Some(PathBuf::from("/home/user")),
            ..Environment::default()
        };
        assert_eq!(
            env.config_path().unwrap(),
            PathBuf::from("/home/user/.config/gpuflo/config.toml")
        );
        assert_eq!(
            env.summary_path().unwrap(),
            PathBuf::from("/home/user/.local/state/gpuflo/daily.json")
        );
    }

    #[test]
    fn missing_and_blank_files_use_built_in_defaults() {
        let env = Environment {
            xdg_config_home: Some(PathBuf::from("/nonexistent-gpuflo-test")),
            ..Environment::default()
        };
        let options = resolve(&env, &cli_defaults()).unwrap();
        assert_eq!(options, PresentationOptions::default());

        let (_dir, env) = temp_config("");
        let options = resolve(&env, &cli_defaults()).unwrap();
        assert_eq!(options, PresentationOptions::default());
    }

    #[test]
    fn toml_overrides_defaults_and_cli_overrides_toml() {
        let (_dir, env) = temp_config("theme = \"nord\"\nmode = \"mini\"\n");
        let options = resolve(&env, &cli_defaults()).unwrap();
        assert_eq!(options.theme, Theme::Nord);
        assert_eq!(options.mode_preference, ModePreference::Mini);

        let cli = CliOptions {
            theme: Some(Theme::Monochrome),
            mode: Some(ModePreference::Compact),
            ..cli_defaults()
        };
        let options = resolve(&env, &cli).unwrap();
        assert_eq!(options.theme, Theme::Monochrome);
        assert_eq!(options.mode_preference, ModePreference::Compact);
    }

    #[test]
    fn any_non_empty_no_color_disables_color() {
        let (_dir, env) = temp_config("");
        let mut env = env;
        env.no_color = Some(std::ffi::OsString::from("1"));
        assert!(!resolve(&env, &cli_defaults()).unwrap().color_enabled);
        env.no_color = Some(std::ffi::OsString::new());
        assert!(resolve(&env, &cli_defaults()).unwrap().color_enabled);

        let (_dir, env) = temp_config("no_color = true\n");
        assert!(!resolve(&env, &cli_defaults()).unwrap().color_enabled);

        let cli = CliOptions {
            no_color: true,
            ..cli_defaults()
        };
        let (_dir, env) = temp_config("");
        assert!(!resolve(&env, &cli).unwrap().color_enabled);
    }

    #[test]
    fn unknown_keys_wrong_types_and_bad_values_are_startup_errors() {
        for content in [
            "them = \"buffalo\"\n",
            "theme = 3\n",
            "theme = \"solarized\"\n",
            "mode = \"full\"\n",
            "no_color = \"yes\"\n",
            "not toml at all [",
        ] {
            let (_dir, env) = temp_config(content);
            let error = resolve(&env, &cli_defaults()).unwrap_err();
            let text = error.to_string();
            assert!(text.contains("config.toml"), "missing path context: {text}");
        }
    }
}
