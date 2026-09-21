//! Keep operational configuration on the canonical GitHub repository.
use crate::{error, Result};
use std::{fs, path::Path, process::Command};

pub const REPOSITORY: &str = "https://github.com/neverhuman/RedlineDB";

fn canonical_remote(url: &str) -> bool {
    let url = url.trim_end_matches('/').trim_end_matches(".git");
    [
        REPOSITORY,
        "git@github.com:neverhuman/RedlineDB",
        "ssh://git@github.com/neverhuman/RedlineDB",
    ]
    .iter()
    .any(|expected| url.eq_ignore_ascii_case(expected))
}

// These records describe past executions. Rewriting them would invalidate their
// hashes. They are never read as operational configuration by this controller.
fn historical(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.contains("/release-evidence/")
        || text.starts_with("docs/archive/")
        || text.contains("/docs/archive/")
        || text.starts_with("docs/migration/")
        || path.file_name().is_some_and(|name| name == "CHANGELOG.md")
        || (text.contains(".jankurai/")
            && matches!(
                path.extension().and_then(|x| x.to_str()),
                Some("json" | "jsonl" | "md")
            ))
}

fn retired_route(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    // Split the literals so this guard does not exempt its own source from scans.
    [
        ["git.neverhuman", ".org"].concat(),
        ["127.0.0.1:", "8787"].concat(),
        ["127.0.0.1:", "8929"].concat(),
        ["localhost:", "8787"].concat(),
        ["localhost:", "8929"].concat(),
        [".jeryu", "/"].concat(),
        ["jeryu ", "access"].concat(),
        ["jeryu", "-signrail"].concat(),
        ["jeryu_install", "_dir"].concat(),
    ]
    .iter()
    .any(|route| text.contains(route))
}

fn files(root: &Path, dir: &Path) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(root)?;
        let name = entry.file_name();
        if [".git", "target", "node_modules", "dist", ".cache", ".agent"]
            .iter()
            .any(|skip| name == *skip)
        {
            continue;
        }
        if name == ".jeryu" {
            return Err(error(format!(
                "retired forge policy: {}",
                relative.display()
            )));
        }
        if name == ".gitlab-ci.yml" {
            return Err(error(format!("retired pipeline: {}", relative.display())));
        }
        if historical(relative) {
            continue;
        }
        if entry.file_type()?.is_symlink() {
            continue; // Component path containment is checked by monorepo validation.
        }
        if entry.file_type()?.is_dir() {
            files(root, &path)?;
        } else if matches!(
            path.extension().and_then(|x| x.to_str()),
            Some("sh" | "toml" | "yaml" | "yml" | "rs")
        ) || matches!(name.to_str(), Some("AGENTS.md" | "Justfile" | "justfile"))
        {
            let text = fs::read_to_string(&path)?;
            if retired_route(&text) {
                return Err(error(format!(
                    "retired forge route: {}",
                    relative.display()
                )));
            }
        }
    }
    Ok(())
}

pub fn validate(root: &Path, manifest: &toml::Value) -> Result<()> {
    if manifest.get("repository").and_then(toml::Value::as_str) != Some(REPOSITORY)
        || manifest.get("authority").and_then(toml::Value::as_str) != Some("single-checkout")
    {
        return Err(error(
            "subrepos.toml must bind the canonical GitHub single checkout",
        ));
    }
    if root.join(".git").exists() {
        let remotes = Command::new("git")
            .arg("-C")
            .arg(root)
            .arg("remote")
            .output()?;
        if !remotes.status.success() {
            return Err(error("cannot inspect Git remotes"));
        }
        for remote in String::from_utf8(remotes.stdout)?.lines() {
            for direction in [vec!["--all"], vec!["--push", "--all"]] {
                let output = Command::new("git")
                    .arg("-C")
                    .arg(root)
                    .args(["remote", "get-url"])
                    .args(direction)
                    .arg(remote)
                    .output()?;
                if !output.status.success()
                    || !String::from_utf8(output.stdout)?
                        .lines()
                        .all(canonical_remote)
                {
                    return Err(error(format!("remote {remote} must use {REPOSITORY}")));
                }
            }
        }
    }
    files(root, root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_https_and_ssh_remotes_only() {
        for url in [
            REPOSITORY,
            "https://github.com/neverhuman/redlineDB.git",
            "git@github.com:neverhuman/RedlineDB.git",
            "ssh://git@github.com/neverhuman/RedlineDB.git",
        ] {
            assert!(canonical_remote(url), "{url}");
        }
        for url in [
            "https://github.com/another-owner/RedlineDB.git",
            "https://github.com/neverhuman/other.git",
            "https://github.com.evil.test/neverhuman/RedlineDB",
            "/local/clone",
        ] {
            assert!(!canonical_remote(url), "{url}");
        }
    }

    #[test]
    fn old_routes_rejected_but_consumer_schema_names_are_data() {
        assert!(retired_route(
            &["https://git.neverhuman", ".org/git/repo"].concat()
        ));
        assert!(retired_route(
            &["http://127.0.0.1:", "8787/git/repo"].concat()
        ));
        assert!(retired_route(
            &["JERYU_INSTALL", "_DIR=/somewhere"].concat()
        ));
        assert!(!retired_route("jeryu-canary-v1"));
        assert!(!retired_route(REPOSITORY));
    }

    #[test]
    fn receipts_are_historical_but_live_configuration_is_checked() {
        assert!(historical(Path::new(
            "subrepos/redline-split-ops/release-evidence/8.0.0/receipt.json"
        )));
        assert!(!historical(Path::new(
            "subrepos/redline-split-ops/repos.manifest.toml"
        )));
        assert!(!historical(Path::new("subrepos/redline-testing/AGENTS.md")));
        assert!(!historical(Path::new(".github/workflows/ci.yml")));
    }
}
