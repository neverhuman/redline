use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

const BACKLOG: &str = include_str!("../../../docs/beyond-sqlite-gaps.md");
const EXPECTED_GAPS: &[&str] = &[
    "Multi-writer / row-locking / queue semantics: `FOR UPDATE`, `SKIP LOCKED`, concurrent row reservations",
    "Migration ergonomics: `ALTER COLUMN`, defaults, constraint add/drop, safer table evolution",
    "Stored SQL routines: SQL functions/procedures, variables, reusable DB-side logic",
    "Replication / sync / CDC: manifest first; executable tests wait for a RedlineDB API",
    "`LISTEN` / `NOTIFY`: Postgres reference, RedlineDB event contract later",
    "Materialized views: create, refresh, indexed refresh targets",
    "Richer typing: decimal, UUID, boolean, timestamps, stricter mode",
    "Unicode/collation/`ILIKE`: start with active Postgres-vs-RedlineDB `ILIKE` tests because RedlineDB already has partial support",
    "JSONB/document indexing: containment, path lookup, indexed generated path cases",
    "Schemas/sequences/identity: namespaces, sequence objects, identity syntax",
    "SQL portability syntax: `MERGE`, `LATERAL`, data-modifying CTEs, `DISTINCT ON`, `DEFAULT` in values",
    "Advanced indexes/search/vector: manifest entries first unless existing implementations already pass",
];

#[derive(Debug)]
struct Gap {
    rank: usize,
    title: String,
    owner: String,
    proof_lane: String,
    sources: Vec<String>,
}

#[test]
fn beyond_sqlite_backlog_is_ranked_and_stable() {
    let gaps = parse_backlog();
    assert_eq!(gaps.len(), EXPECTED_GAPS.len(), "unexpected backlog length");
    for (index, gap) in gaps.iter().enumerate() {
        assert_eq!(gap.rank, index + 1, "rank sequence drifted");
        assert_eq!(gap.title, EXPECTED_GAPS[index], "gap title drifted");
    }
}

#[test]
fn beyond_sqlite_backlog_sources_all_committed_tips() {
    let repo = repo_root();
    let source_files = parse_backlog()
        .into_iter()
        .flat_map(|gap| gap.sources)
        .collect::<BTreeSet<_>>();
    let committed_tips = fs::read_dir(repo.join("tips/beyond"))
        .expect("read tips/beyond")
        .map(|entry| {
            entry
                .expect("tip entry")
                .file_name()
                .into_string()
                .expect("utf8 tip filename")
        })
        .filter(|name| name.ends_with(".txt"))
        .collect::<BTreeSet<_>>();

    assert_eq!(
        source_files, committed_tips,
        "docs/beyond-sqlite-gaps.md must cite every tips/beyond/*.txt source"
    );
}

#[test]
fn beyond_sqlite_backlog_maps_to_known_owners_and_lanes() {
    let repo = repo_root();
    let owners = owner_names(&repo);
    let lanes = proof_lane_names(&repo);

    for gap in parse_backlog() {
        assert!(
            owners.contains(gap.owner.as_str()),
            "unknown owner `{}` for rank {}",
            gap.owner,
            gap.rank
        );
        assert!(
            lanes.contains(gap.proof_lane.as_str()),
            "unknown proof lane `{}` for rank {}",
            gap.proof_lane,
            gap.rank
        );
    }
}

fn parse_backlog() -> Vec<Gap> {
    BACKLOG
        .lines()
        .filter_map(parse_table_row)
        .collect::<Vec<_>>()
}

fn parse_table_row(line: &str) -> Option<Gap> {
    let trimmed = line.trim();
    if !trimmed.starts_with('|') || !trimmed.ends_with('|') {
        return None;
    }
    let cells = trimmed
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect::<Vec<_>>();
    if cells.len() < 6 {
        return None;
    }
    let rank = cells[0].parse::<usize>().ok()?;
    let sources = cells[4]
        .split(',')
        .map(|source| source.trim().trim_matches('`').to_owned())
        .filter(|source| !source.is_empty())
        .collect::<Vec<_>>();
    Some(Gap {
        rank,
        title: cells[1].to_owned(),
        owner: cells[2].to_owned(),
        proof_lane: cells[3].to_owned(),
        sources,
    })
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("repo root")
        .to_path_buf()
}

fn owner_names(repo: &std::path::Path) -> BTreeSet<String> {
    let text = fs::read_to_string(repo.join(".jankurai/owner-map.json")).expect("owner map");
    let value: serde_json::Value = serde_json::from_str(&text).expect("owner map JSON");
    value
        .get("owners")
        .and_then(serde_json::Value::as_object)
        .expect("owners object")
        .values()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn proof_lane_names(repo: &std::path::Path) -> BTreeSet<String> {
    let text = fs::read_to_string(repo.join(".jankurai/proof-lanes.toml")).expect("proof lanes");
    let mut lanes = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(name) = line.strip_prefix("name = ") {
            lanes.insert(name.trim_matches('"').to_owned());
        } else if line.starts_with('[') && line.ends_with(']') && !line.starts_with("[[") {
            lanes.insert(line.trim_matches(['[', ']']).to_owned());
        }
    }
    lanes
}

// ---------------------------------------------------------------------------
// PostgreSQL lane policy: the skip-list mirrors, the regression policy's
// declared rejections, and the capability matrix (L-10 / PG-04).
// ---------------------------------------------------------------------------

const PG_MANIFEST: &str = "subrepos/redline-testing/corpus/beyond_sqlite/generated_manifest.json";
const SKIP_LIST_MIRRORS: [&str; 3] = [
    "metadata/beyond_sqlite/skip-list.toml",
    "subrepos/redline-testing/metadata/beyond_sqlite/skip-list.toml",
    "subrepos/redline/metadata/beyond_sqlite/skip-list.toml",
];

fn pg_cases() -> Vec<serde_json::Value> {
    let text = fs::read_to_string(repo_root().join(PG_MANIFEST)).expect("PG manifest");
    serde_json::from_str(&text).expect("PG manifest JSON")
}

fn pg_case_id(case: &serde_json::Value) -> String {
    format!("BEYOND-CASE-{:05}", case["id"].as_u64().expect("case id"))
}

/// The cases whose corpus entry declares the error text the target must print.
fn corpus_declared_rejections() -> BTreeSet<String> {
    pg_cases()
        .iter()
        .filter(|case| case["expected_target_stderr_contains"].is_string())
        .map(pg_case_id)
        .collect()
}

fn read_json(path: &str) -> serde_json::Value {
    let text =
        fs::read_to_string(repo_root().join(path)).unwrap_or_else(|err| panic!("{path}: {err}"));
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("{path}: {err}"))
}

/// `(case_id, name)` of every `[[skip]]` entry, in file order.
fn skip_entries(text: &str) -> Vec<(String, String)> {
    let value = |line: &str| {
        line.split_once('=')
            .map(|(_, value)| value.trim().trim_matches('"').to_owned())
    };
    let mut entries: Vec<(Option<String>, Option<String>)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if line == "[[skip]]" {
            entries.push((None, None));
        } else if let Some(entry) = entries.last_mut() {
            if line.starts_with("case_id") {
                entry.0 = value(line);
            } else if line.starts_with("name") {
                entry.1 = value(line);
            }
        }
    }
    entries
        .into_iter()
        .map(|(id, name)| (id.expect("skip case_id"), name.expect("skip name")))
        .collect()
}

#[test]
fn skip_list_mirrors_are_identical_and_cover_real_cases() {
    let repo = repo_root();
    let texts: Vec<String> = SKIP_LIST_MIRRORS
        .iter()
        .map(|path| {
            fs::read_to_string(repo.join(path)).unwrap_or_else(|err| panic!("{path}: {err}"))
        })
        .collect();
    for (path, text) in SKIP_LIST_MIRRORS.iter().zip(&texts).skip(1) {
        assert_eq!(
            text, &texts[0],
            "{path} differs from {}",
            SKIP_LIST_MIRRORS[0]
        );
    }
    let names: std::collections::BTreeMap<String, String> = pg_cases()
        .iter()
        .map(|case| {
            (
                pg_case_id(case),
                case["name"].as_str().expect("name").to_owned(),
            )
        })
        .collect();
    let entries = skip_entries(&texts[0]);
    assert!(entries.len() > 100, "skip list shrank: {}", entries.len());
    for (case_id, name) in entries {
        let id = format!("BEYOND-CASE-{case_id}");
        assert_eq!(
            names.get(&id),
            Some(&name),
            "skip entry {case_id} ({name}) does not name a corpus case by its manifest name"
        );
    }
}

#[test]
fn regression_policy_declares_exactly_the_corpus_rejections() {
    let policy = read_json("metadata/beyond_sqlite/postgres-regression.json");
    let declared: BTreeSet<String> = policy["declared_rejections"]
        .as_array()
        .expect("postgres-regression.json lists declared_rejections")
        .iter()
        .map(|id| id.as_str().expect("case id").to_owned())
        .collect();
    let corpus = corpus_declared_rejections();
    assert_eq!(
        corpus.len(),
        12,
        "the corpus declares {} rejections",
        corpus.len()
    );
    assert_eq!(declared, corpus);
    // The count lives in the array, not in prose that can drift from it.
    let reason = policy["reason"]
        .as_str()
        .expect("reason")
        .to_ascii_lowercase();
    for word in ["nine", "twelve"] {
        assert!(!reason.contains(word), "reason hardcodes a count: {reason}");
    }
    assert!(
        !reason.chars().any(|c| c.is_ascii_digit()),
        "reason hardcodes a count: {reason}"
    );
}

const REQUIRED_CAPABILITIES: [&str; 12] = [
    "wire",
    "tls",
    "roles",
    "sqlstate",
    "notify_delivery",
    "advisory_locks",
    "wal_lsn",
    "snapshot_export",
    "logical_replication",
    "vector",
    "citext",
    "multi_session",
];

#[test]
fn capability_matrix_covers_wire_tls_roles_sqlstate() {
    let matrix = read_json("metadata/beyond_sqlite/postgres-capabilities.json");
    assert_eq!(matrix["schema_version"], "redline-postgres-capabilities-v1");
    let corpus: BTreeSet<String> = pg_cases().iter().map(pg_case_id).collect();
    let rows = matrix["capabilities"]
        .as_array()
        .expect("capabilities array");
    let mut seen = BTreeSet::new();
    for row in rows {
        let capability = row["capability"].as_str().expect("capability");
        assert!(
            seen.insert(capability.to_owned()),
            "{capability} listed twice"
        );
        let status = row["status"].as_str().expect("status");
        assert!(
            matches!(
                status,
                "implemented" | "partial" | "unsupported" | "unverified"
            ),
            "{capability}: unknown status {status}"
        );
        assert!(
            !row["surface"].as_str().unwrap_or_default().is_empty(),
            "{capability}: surface"
        );
        assert!(
            !row["note"].as_str().unwrap_or_default().is_empty(),
            "{capability}: note"
        );
        for case in row["cases"].as_array().expect("cases array") {
            let case = case.as_str().expect("case id");
            assert!(
                corpus.contains(case),
                "{capability} cites unknown case {case}"
            );
        }
    }
    for capability in REQUIRED_CAPABILITIES {
        assert!(
            seen.contains(capability),
            "capability matrix lacks {capability}"
        );
    }
    let status = |name: &str| {
        rows.iter()
            .find(|row| row["capability"] == name)
            .and_then(|row| row["status"].as_str())
            .unwrap_or_default()
            .to_owned()
    };
    // The SQL-shell corpus cannot establish any of these, and a rejection
    // test or a stand-in is not support.
    for name in [
        "wire",
        "tls",
        "roles",
        "sqlstate",
        "notify_delivery",
        "wal_lsn",
        "snapshot_export",
        "logical_replication",
        "vector",
        "multi_session",
    ] {
        assert!(
            matches!(status(name).as_str(), "unsupported" | "unverified"),
            "{name} is marked {}",
            status(name)
        );
    }
    assert_eq!(status("citext"), "partial");
    // PG-01 made them real within one process; the shared and
    // transaction-level variants and cross-process locks are still missing.
    assert_eq!(status("advisory_locks"), "partial");
}

#[test]
fn skip_doc_renders_the_capability_matrix() {
    let matrix = read_json("metadata/beyond_sqlite/postgres-capabilities.json");
    let doc =
        fs::read_to_string(repo_root().join("docs/beyond-postgres-skips.md")).expect("skip doc");
    assert!(doc.contains("metadata/beyond_sqlite/postgres-capabilities.json"));
    for row in matrix["capabilities"].as_array().expect("capabilities") {
        let capability = row["capability"].as_str().expect("capability");
        let status = row["status"].as_str().expect("status");
        let rendered = format!("| `{capability}` | {status} |");
        assert!(
            doc.contains(&rendered),
            "docs/beyond-postgres-skips.md lacks row {rendered}"
        );
    }
}

#[test]
fn postgres_docs_do_not_hardcode_a_stale_rejection_count() {
    let declared = corpus_declared_rejections();
    for path in [
        "docs/beyond-postgres-skips.md",
        "docs/manual/03-for-agents.md",
        "docs/manual/05-postgres-coverage.md",
        "docs/manual/06-sql-you-will-write.md",
        "docs/manual/10-limits.md",
        "docs/manual/README.md",
        "docs/manual/appendix-coverage.md",
    ] {
        let text = fs::read_to_string(repo_root().join(path))
            .expect(path)
            .to_ascii_lowercase();
        for word in ["nine", "256 passes"] {
            assert!(!text.contains(word), "{path} still says {word:?}");
        }
    }
    // Every declared rejection is named in the ledger and the coverage page.
    for path in [
        "docs/manual/appendix-coverage.md",
        "docs/manual/05-postgres-coverage.md",
    ] {
        let text = fs::read_to_string(repo_root().join(path)).expect(path);
        for id in &declared {
            let number = id.trim_start_matches("BEYOND-CASE-");
            assert!(text.contains(number), "{path} does not list case {number}");
        }
    }
}
