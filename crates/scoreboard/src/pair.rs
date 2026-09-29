//! The durability pairs: each RedlineDB mode against the SQLite setting
//! that promises the same.

use std::fmt;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Pair {
    /// RedlineDB `Normal` against SQLite WAL with `synchronous=NORMAL`: a
    /// commit survives a process crash, not a power loss.
    Normal,
    /// RedlineDB `Strict` against SQLite WAL with `synchronous=FULL`: every
    /// commit is synced before it returns.
    Strict,
}

impl Pair {
    pub fn sqlite_synchronous(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Strict => "FULL",
        }
    }
}

impl fmt::Display for Pair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Normal => "normal",
            Self::Strict => "strict",
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Redline,
    Sqlite,
}

impl fmt::Display for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Redline => "redline",
            Self::Sqlite => "sqlite",
        })
    }
}
