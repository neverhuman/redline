//! GitHub single-checkout component validation entrypoint.
mod authority;
mod monorepo;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug)]
struct RepairError {
    reason: String,
}

impl std::fmt::Display for RepairError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter,
            "purpose=validate the GitHub single checkout; reason={}; docs_url=docs/testing.md; repair_hint=correct the named input and rerun redlinectl validate; common_fixes=use the complete GitHub checkout | restore the declared component path | use the canonical GitHub fetch and push remote",
            self.reason)
    }
}

impl std::error::Error for RepairError {}

fn error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(RepairError {
        reason: message.into(),
    })
}

fn main() {
    let result = monorepo::root()
        .ok_or_else(|| error("run from the complete https://github.com/neverhuman/redline checkout, or set REDLINE_REPO_ROOT"))
        .and_then(|root| monorepo::run(&root));
    if let Err(error) = result {
        eprintln!("redline-proof: {error}");
        std::process::exit(1);
    }
}
