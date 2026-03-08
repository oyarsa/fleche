//! Output formatting for CLI commands.
//!
//! Provides [`OutputFormat`] to handle the dual human/JSON output pattern
//! without scattering `if json` checks throughout the codebase.

use serde::Serialize;

/// Controls how command output is rendered.
#[derive(Debug, Clone, Copy, Default)]
pub enum OutputFormat {
    /// Human-readable styled output (the default).
    #[default]
    Human,
    /// Machine-readable JSON output.
    Json,
}

impl OutputFormat {
    /// Returns `true` when JSON output is active.
    pub fn is_json(self) -> bool {
        matches!(self, Self::Json)
    }

    /// Prints `data` as pretty JSON, or calls `human` to print styled output.
    ///
    /// ```ignore
    /// format.print(&jobs, || {
    ///     print_job_table(&jobs);
    ///     Ok(())
    /// })?;
    /// ```
    pub fn print<T, E>(self, data: &T, human: impl FnOnce() -> Result<(), E>) -> Result<(), E>
    where
        T: Serialize,
        E: From<serde_json::Error>,
    {
        match self {
            Self::Human => human(),
            Self::Json => {
                println!("{}", serde_json::to_string_pretty(data)?);
                Ok(())
            }
        }
    }

    /// Prints `data` as pretty JSON when in JSON mode; does nothing for human.
    ///
    /// Useful when human output is handled separately (e.g. printed
    /// incrementally in a loop) and only JSON needs a final dump.
    pub fn print_json_only<T, E>(self, data: &T) -> Result<(), E>
    where
        T: Serialize,
        E: From<serde_json::Error>,
    {
        if matches!(self, Self::Json) {
            println!("{}", serde_json::to_string_pretty(data)?);
        }
        Ok(())
    }
}
