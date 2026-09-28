mod args;
mod cmds;
mod run;

pub use args::Cli;

use anyhow::Result;
use args::CommandKind;

pub fn run(cli: Cli) -> Result<()> {
    match cli.command {
        CommandKind::Run(args) => run::run_suite(args),
        CommandKind::CheckPostgres(args) => crate::beyond_sqlite::gate::check(
            &args.input,
            args.baseline.as_deref(),
            args.readme.as_deref(),
            &args.publication.policy(),
        ),
        CommandKind::CheckSqlite(args) => {
            crate::report::check_sqlite(crate::report::CheckSqliteOptions {
                official_evidence: args.official_evidence,
                output: args.output,
            })
        }
        CommandKind::Report(args) => cmds::report(args),
        CommandKind::List(args) => cmds::list(args),
        CommandKind::JankuraiCompare(args) => cmds::jankurai_compare(args),
        CommandKind::Sentinel(args) => cmds::sentinel(args),
        CommandKind::Version => {
            println!("redline-testing {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}
