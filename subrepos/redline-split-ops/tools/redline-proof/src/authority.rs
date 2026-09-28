//! Keep operational configuration on the canonical GitHub repository.
use crate::{error, Result};
use std::{fs, path::Path, process::Command};

pub const REPOSITORY: &str = "https://github.com/neverhuman/redline";

fn canonical_remote(url: &str) -> bool {
    let url = url.trim_end_matches('/').trim_end_matches(".git");
    [
        REPOSITORY,
        "git@github.com:neverhuman/redline",
        "ssh://git@github.com/neverhuman/redline",
    ]
    .iter()
    .any(|expected| url.eq_ignore_ascii_case(expected))
}

// These records describe past executions. Rewriting them would invalidate their
// hashes. They are never read as operational configuration by this controller.
// `subrepos/redline/` is the imported historical hub and `tips/` holds planning
// records; both quote the retired repository name as history.
fn historical(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.contains("/release-evidence/")
        || text.starts_with("docs/archive/")
        || text.contains("/docs/archive/")
        || text.starts_with("docs/migration/")
        || text.starts_with("subrepos/redline/")
        || text.starts_with("tips/")
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
        // The retired repository; GitHub names are case-insensitive and the
        // old name now resolves to a different owner's repository.
        ["neverhuman/", "redlinedb"].concat(),
        ["neverhumanbot/", "redlinedb"].concat(),
    ]
    .iter()
    .any(|route| text.contains(route))
}

// Readers copy install, clone and dependency commands from these pages.
fn user_doc(path: &Path) -> bool {
    let text = path.to_string_lossy();
    path.extension().and_then(|x| x.to_str()) == Some("md")
        && (path.file_name().is_some_and(|name| name == "README.md")
            || text.starts_with("docs/")
            || text.contains("/docs/"))
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
            || user_doc(relative)
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

    // The retired repository name, split so this source passes its own scan.
    fn legacy(owner: &str) -> String {
        [owner, "/", "Redline", "DB"].concat()
    }

    #[test]
    fn canonical_https_and_ssh_remotes_only() {
        for url in [
            REPOSITORY,
            "https://github.com/neverhuman/redline.git",
            "https://github.com/neverhuman/redline/",
            "git@github.com:neverhuman/redline.git",
            "ssh://git@github.com/neverhuman/redline.git",
        ] {
            assert!(canonical_remote(url), "{url}");
        }
        for url in [
            format!("https://github.com/{}.git", legacy("neverhuman")),
            format!("https://github.com/{}", legacy("neverhuman").to_lowercase()),
            format!("git@github.com:{}.git", legacy("neverhuman")),
            format!("ssh://git@github.com/{}.git", legacy("neverhuman")),
            format!("https://github.com/{}.git", legacy("neverhumanbot")),
            "https://github.com/another-owner/redline.git".into(),
            "https://github.com/neverhuman/redline-core.git".into(),
            "https://github.com/neverhuman/other.git".into(),
            "https://github.com.evil.test/neverhuman/redline".into(),
            "/local/clone".into(),
        ] {
            assert!(!canonical_remote(&url), "{url}");
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
    fn retired_repository_name_is_an_old_route_in_any_case() {
        for text in [
            format!(
                "repository = \"https://github.com/{}\"",
                legacy("neverhuman")
            ),
            format!(
                "git clone https://github.com/{}",
                legacy("neverhuman").to_lowercase()
            ),
            format!("gh pr list --repo {}", legacy("NEVERHUMAN")),
            format!("https://github.com/{}/releases", legacy("neverhumanbot")),
        ] {
            assert!(retired_route(&text), "{text}");
        }
        for text in [
            REPOSITORY,
            "https://github.com/neverhuman/redline.git",
            "https://github.com/neverhuman/redline-core.git",
            "https://github.com/neverhuman/redline/releases/download/v5.0.0/redlinedb.tar.gz",
            "redlinedb = { git = \"https://github.com/neverhuman/redline\" }",
        ] {
            assert!(!retired_route(text), "{text}");
        }
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

    #[test]
    fn readme_and_docs_pages_are_scanned() {
        for page in [
            "README.md",
            "docs/install.md",
            "docs/manual/02-start-here.md",
            "subrepos/redline-web/README.md",
            "subrepos/redline-testing/docs/release.md",
        ] {
            assert!(user_doc(Path::new(page)), "{page}");
        }
        for page in [
            "GROK_GAPS.md",
            "docs/manual/example.sql",
            "subrepos/x/NOTES.md",
        ] {
            assert!(!user_doc(Path::new(page)), "{page}");
        }
    }

    #[test]
    fn historical_hub_and_planning_tips_are_records() {
        assert!(historical(Path::new("subrepos/redline/AGENTS.md")));
        assert!(historical(Path::new("subrepos/redline/ops/ci/lib.sh")));
        assert!(historical(Path::new("tips/phases/00-phase-index.md")));
        assert!(historical(Path::new("tips/release/plan.toml")));
        assert!(!historical(Path::new("subrepos/redline-web/install.sh")));
        assert!(!historical(Path::new(
            "subrepos/redline-testing/ops/deploy/telemetry.sh"
        )));
        assert!(!historical(Path::new("scripts/tips/run.sh")));
    }
}
