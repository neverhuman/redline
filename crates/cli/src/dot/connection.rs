//! `.connection`: the sqlite3 shell's five database connection slots.
//!
//! `.connection` lists the open slots, `.connection N` makes slot N active
//! (opening a fresh in-memory database there when it is empty) and keeps the
//! database that was active in its own slot, and `.connection close N`
//! closes an inactive slot. sqlite3 opens a slot's database lazily and shows
//! one opened that way as `(memory)`; RedlineDB opens it at once, so it lists
//! as `:memory:` and a slot left before any statement ran is still listed.

use std::path::PathBuf;

use redlinedb::{Connection, Database};

use super::{CliState, DotOutcome};

/// sqlite3's `ArraySize(p->aAuxDb)`.
pub const SLOTS: usize = 5;

/// A connection slot that is not active.
pub struct Slot {
    db: Database,
    conn: Connection,
    db_path: PathBuf,
}

const USAGE: &str = "Usage: .connection [close] [CONNECTION-NUMBER]";

pub fn connection(state: &mut CliState, args: &[&str]) -> Result<DotOutcome, String> {
    match args {
        [] => list(state),
        [slot] => switch(state, slot_number(slot)?),
        [close, slot] if *close == "close" => close_slot(state, slot_number(slot)?),
        _ => Err(USAGE.to_owned()),
    }
}

/// A single digit, as sqlite3 requires; a digit past the last slot is
/// accepted and ignored, as there.
fn slot_number(arg: &str) -> Result<usize, String> {
    match arg.as_bytes() {
        [digit] if digit.is_ascii_digit() => Ok(usize::from(digit - b'0')),
        _ => Err(USAGE.to_owned()),
    }
}

fn list(state: &mut CliState) -> Result<DotOutcome, String> {
    for index in 0..SLOTS {
        let line = if index == state.active_slot {
            format!("ACTIVE {index}: {}", state.db_path.display())
        } else if let Some(slot) = &state.slots[index] {
            format!("       {index}: {}", slot.db_path.display())
        } else {
            continue;
        };
        state
            .output
            .write_line(&line)
            .map_err(|err| err.to_string())?;
    }
    Ok(DotOutcome::Ok)
}

fn switch(state: &mut CliState, index: usize) -> Result<DotOutcome, String> {
    if index >= SLOTS || index == state.active_slot {
        return Ok(DotOutcome::Ok);
    }
    let incoming = match state.slots[index].take() {
        Some(slot) => slot,
        None => {
            let db = Database::create_in_memory(crate::cli_open_options())
                .map_err(|err| err.to_string())?;
            let conn = db.connect().map_err(|err| err.to_string())?;
            Slot {
                db,
                conn,
                db_path: PathBuf::from(":memory:"),
            }
        }
    };
    let outgoing = Slot {
        db: std::mem::replace(&mut state.db, incoming.db),
        conn: std::mem::replace(&mut state.conn, incoming.conn),
        db_path: std::mem::replace(&mut state.db_path, incoming.db_path),
    };
    state.slots[state.active_slot] = Some(outgoing);
    state.active_slot = index;
    Ok(DotOutcome::Ok)
}

fn close_slot(state: &mut CliState, index: usize) -> Result<DotOutcome, String> {
    if index == state.active_slot {
        return Err("cannot close the active database connection".to_owned());
    }
    if let Some(slot) = state.slots.get_mut(index) {
        *slot = None;
    }
    Ok(DotOutcome::Ok)
}
