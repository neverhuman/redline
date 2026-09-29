//! `redline-scoreboard`; see the library documentation.

// Rust-side allocations of both engines go through mimalloc, the allocator
// the `redlinedb` shell ships with. SQLite's own C allocations use the
// system allocator.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> anyhow::Result<()> {
    redlinedb_scoreboard::cli::main()
}
