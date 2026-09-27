use std::fmt;
use std::str::FromStr;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Priority {
    P0,
    P1,
    P2,
    P3,
    P4,
}

impl fmt::Display for Priority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::P0 => "P0",
            Self::P1 => "P1",
            Self::P2 => "P2",
            Self::P3 => "P3",
            Self::P4 => "P4",
        })
    }
}

impl FromStr for Priority {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_uppercase().as_str() {
            "P0" => Ok(Self::P0),
            "P1" => Ok(Self::P1),
            "P2" => Ok(Self::P2),
            "P3" => Ok(Self::P3),
            "P4" => Ok(Self::P4),
            other => bail!("unknown priority `{other}`"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Memory,
    Tempfile,
    Catalog,
    ExternalApp,
    SideEffect,
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Memory => "memory",
            Self::Tempfile => "tempfile",
            Self::Catalog => "catalog",
            Self::ExternalApp => "external_app",
            Self::SideEffect => "side_effect",
        })
    }
}

impl FromStr for Profile {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "memory" | "mem" => Ok(Self::Memory),
            "tempfile" | "file" | "tmp" => Ok(Self::Tempfile),
            "catalog" => Ok(Self::Catalog),
            "external_app" | "external-app" => Ok(Self::ExternalApp),
            "side_effect" | "side-effect" => Ok(Self::SideEffect),
            other => bail!("unknown profile `{other}`"),
        }
    }
}

/// How the differential compares what the two shells printed (SQ-06).
/// Declared diagnostics (the `expected_*_contains` fragments and
/// `expected_stdout`) are matched on normalized text whatever the mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonMode {
    /// Byte for byte: nothing decoded, trimmed or folded.
    #[default]
    CliBytesExact,
    /// CRLF read as LF and nothing else, for a case about platform line
    /// framing whose subject is not the framing bytes themselves.
    CliTextLf,
}

impl ComparisonMode {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Case {
    pub id: usize,
    pub folder: String,
    pub name: String,
    pub category: String,
    pub priority: Priority,
    pub profile: Profile,
    pub kind: String,
    pub description: String,
    pub status: String,
    pub db: String,
    pub args: Vec<String>,
    pub stdin: String,
    pub expected_exit: i32,
    pub compare_stdout: bool,
    pub expected_stdout: Option<String>,
    pub expected_stdout_contains: Vec<String>,
    pub expected_stderr_contains: Vec<String>,
    pub expected_combined_contains: Vec<String>,
    pub files: Vec<(String, String)>,
    pub script: Option<String>,
    pub notes: String,
    /// Capability tokens required to run this case. Resolved against the
    /// reference shell at probe time; cases missing a capability are skipped
    /// with a clear "<reference shell X.Y.Z> lacks <feature>" reason. Backwards
    /// compatible: the pinned manifest omits the field and serde fills it in
    /// as an empty vec.
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    /// How stdout and stderr are compared; `cli_bytes_exact` when absent.
    #[serde(default, skip_serializing_if = "ComparisonMode::is_default")]
    pub comparison_mode: ComparisonMode,
    /// Lines (on either stream, either shell) that start with one of these
    /// are left out of the comparison: output the case cannot pin down,
    /// such as a random seed in a trace. Every other byte is compared.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ignore_line_prefixes: Vec<String>,
    /// Why `compare_stdout` is false: what in stdout is inherently
    /// engine-specific. Required for every such case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdout_uncompared_reason: Option<String>,
}

impl Case {
    pub fn display_id(&self) -> String {
        format!("{:05}", self.id)
    }

    pub fn case_file_name(&self) -> String {
        format!("{}.rs", self.folder)
    }
}
