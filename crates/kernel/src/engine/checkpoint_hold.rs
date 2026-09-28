//! A checkpoint that keeps every later checkpoint waiting, for callers that
//! copy the database files and need them to stay one consistent cut.

use std::sync::MutexGuard;

use crate::storage::ControlFile;
use crate::{Error, Result};

use super::{CheckpointStats, Engine};

/// While this lives, no checkpoint runs: an explicit one waits, and so does
/// a pressure checkpoint a writer asks for (that writer waits with it).
/// Without checkpoints nothing writes a logged page to the page file,
/// rewrites a control slot or prunes the WAL, so a copy taken meanwhile has
/// a page file, control files and WAL that agree.
#[must_use = "the hold ends when it is dropped"]
pub struct CheckpointHold<'a> {
    _serial: MutexGuard<'a, ()>,
}

impl Engine {
    /// Checkpoint, and hold off every other checkpoint until the returned
    /// hold drops. Physical backups and snapshot copies take this before
    /// they read the files.
    pub fn checkpoint_and_hold(&self) -> Result<(CheckpointStats, CheckpointHold<'_>)> {
        let serial = self
            .checkpoint_serial
            .lock()
            .map_err(|_| Error::CorruptPage("checkpoint serial mutex poisoned"))?;
        let stats = if self.volatile {
            CheckpointStats {
                control: ControlFile::default(),
                flushed_pages: 0,
                flush_batches: 0,
            }
        } else {
            self.checkpoint_serialized(&serial)?
        };
        Ok((stats, CheckpointHold { _serial: serial }))
    }
}
