use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use super::bounded::{self, Captured, ExecutionOutcome, Limits};
use super::case::{Case, Profile};
use super::memory::ProcessMemory;
use super::runner::VerdictReason;
use super::scope_policy::{GapKind, SCOPE_POLICY_PATH, ScopePolicy};
use super::text::sanitize_identifier;

/// Canonical name of the SQLite reference CLI that the harness drives over a
/// subprocess (never an in-process DB driver). Centralized here, in the
/// subprocess engine module, so the rest of the crate refers to it by symbol.
pub const REFERENCE_CLI_BIN: &str = "sqlite3";

#[derive(Debug, Clone)]
pub struct EngineSpec {
    pub name: String,
    pub bin: PathBuf,
    /// The deadline and output cap every run of a case is held to (SQ-09).
    pub limits: Limits,
    identity: Arc<OnceLock<Result<BinaryIdentity, String>>>,
}

#[derive(Debug, Clone)]
pub struct BinaryIdentity {
    pub executable_path: String,
    pub executable_sha256: String,
    pub version: String,
}

#[derive(Debug, Clone)]
pub struct EngineOutput {
    pub engine: String,
    pub executable_path: String,
    pub executable_sha256: String,
    pub version: String,
    pub status_code: Option<i32>,
    pub elapsed: Duration,
    /// Exactly the bytes the child wrote, never decoded (SQ-06).
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub memory_status: String,
    pub peak_rss_kb: Option<u64>,
    pub rss_sampled_kb: Option<u64>,
    /// How the run ended (SQ-09).
    pub outcome: ExecutionOutcome,
    /// Why a run that is not `outcome.is_complete()` left no whole result.
    pub failure: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    PercentileFunctions,
    DotCrlf,
    DotDbInfo,
    DotDbTotxt,
    DotRecover,
    EscapeSymbolOption,
    Fts5,
    Rtree,
    Dbstat,
    Jsonb,
    Math1,
    GenerateSeries,
    JsonPretty,
    JsonbArrayInsert,
    Regexp,
}

impl Capability {
    pub fn description(self) -> &'static str {
        match self {
            Self::PercentileFunctions => "median()/percentile_cont()",
            Self::DotCrlf => ".crlf",
            Self::DotDbInfo => ".dbinfo",
            Self::DotDbTotxt => ".dbtotxt",
            Self::DotRecover => ".recover",
            Self::EscapeSymbolOption => "-escape symbol",
            Self::Fts5 => "fts5 virtual table",
            Self::Rtree => "rtree virtual table",
            Self::Dbstat => "dbstat virtual table",
            Self::Jsonb => "jsonb() (3.45+)",
            Self::Math1 => "math1 functions (acos/asin/sqrt/etc.)",
            Self::GenerateSeries => "generate_series virtual table",
            Self::JsonPretty => "json_pretty() (3.46+)",
            Self::JsonbArrayInsert => "jsonb_array_insert() (3.47+)",
            Self::Regexp => "the REGEXP operator",
        }
    }

    pub fn from_token(token: &str) -> Option<Self> {
        match token {
            "percentile_functions" => Some(Self::PercentileFunctions),
            "dot_crlf" => Some(Self::DotCrlf),
            "dot_dbinfo" => Some(Self::DotDbInfo),
            "dot_dbtotxt" => Some(Self::DotDbTotxt),
            "dot_recover" => Some(Self::DotRecover),
            "escape_symbol_option" => Some(Self::EscapeSymbolOption),
            "fts5" => Some(Self::Fts5),
            "rtree" => Some(Self::Rtree),
            "dbstat" => Some(Self::Dbstat),
            "jsonb" => Some(Self::Jsonb),
            "math1" => Some(Self::Math1),
            "generate_series" => Some(Self::GenerateSeries),
            "json_pretty" => Some(Self::JsonPretty),
            "jsonb_array_insert" => Some(Self::JsonbArrayInsert),
            "regexp" => Some(Self::Regexp),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShellCapabilities {
    pub version: String,
    pub percentile_functions: bool,
    pub dot_crlf: bool,
    pub dot_dbinfo: bool,
    pub dot_dbtotxt: bool,
    pub dot_recover: bool,
    pub escape_symbol_option: bool,
    pub fts5: bool,
    pub rtree: bool,
    pub dbstat: bool,
    pub jsonb: bool,
    pub math1: bool,
    pub generate_series: bool,
    pub json_pretty: bool,
    pub jsonb_array_insert: bool,
    pub regexp: bool,
}

impl ShellCapabilities {
    pub fn supports(&self, capability: Capability) -> bool {
        match capability {
            Capability::PercentileFunctions => self.percentile_functions,
            Capability::DotCrlf => self.dot_crlf,
            Capability::DotDbInfo => self.dot_dbinfo,
            Capability::DotDbTotxt => self.dot_dbtotxt,
            Capability::DotRecover => self.dot_recover,
            Capability::EscapeSymbolOption => self.escape_symbol_option,
            Capability::Fts5 => self.fts5,
            Capability::Rtree => self.rtree,
            Capability::Dbstat => self.dbstat,
            Capability::Jsonb => self.jsonb,
            Capability::Math1 => self.math1,
            Capability::GenerateSeries => self.generate_series,
            Capability::JsonPretty => self.json_pretty,
            Capability::JsonbArrayInsert => self.jsonb_array_insert,
            Capability::Regexp => self.regexp,
        }
    }
}

/// A case not run because the scope policy lists its gap (SQ-05).
#[derive(Debug)]
pub struct SkippedCase {
    pub case: Case,
    pub reason: String,
    /// The `scope-policy.json` exception that allows the skip.
    pub policy_exception_id: String,
}

/// A case that cannot run and that no exception covers: it fails, with
/// `reference_capability_missing` or `target_unsupported`.
#[derive(Debug)]
pub struct RejectedCase {
    pub case: Case,
    pub verdict_reason: VerdictReason,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct CasePartition {
    pub runnable: Vec<Case>,
    pub skipped: Vec<SkippedCase>,
    pub rejected: Vec<RejectedCase>,
}

impl CasePartition {
    /// Files a case the target side cannot run: skipped when `policy`
    /// lists this gap for it in `suite`, rejected otherwise.
    pub fn target_gap(
        &mut self,
        policy: &ScopePolicy,
        suite: &str,
        case: Case,
        kind: GapKind,
        reason: String,
    ) {
        match policy.covers(suite, &case, kind) {
            Some(policy_exception_id) => self.skipped.push(SkippedCase {
                case,
                reason,
                policy_exception_id,
            }),
            None => self.rejected.push(RejectedCase {
                case,
                verdict_reason: VerdictReason::TargetUnsupported,
                reason: format!("{reason}; {SCOPE_POLICY_PATH} lists no exception for it"),
            }),
        }
    }
}

/// Splits `cases` by what both shells can run (SQ-05). A case whose
/// declared capability the reference lacks fails: agreement with such a
/// reference proves nothing, and no exception can cover it. A case whose
/// capability the target lacks is skipped only when the scope policy lists
/// it for `suite`, and fails otherwise.
pub fn partition_cases(
    cases: Vec<Case>,
    reference_caps: &ShellCapabilities,
    target_caps: &ShellCapabilities,
    policy: &ScopePolicy,
    suite: &str,
) -> Result<CasePartition> {
    let mut partition = CasePartition::default();
    for case in cases {
        let required = required_capabilities(&case)?;
        if let Some(capability) = required
            .iter()
            .find(|capability| !reference_caps.supports(**capability))
        {
            partition.rejected.push(RejectedCase {
                reason: format!(
                    "reference sqlite3 {} lacks {}",
                    reference_caps.version,
                    capability.description()
                ),
                verdict_reason: VerdictReason::ReferenceCapabilityMissing,
                case,
            });
        } else if let Some(capability) = required
            .iter()
            .find(|capability| !target_caps.supports(**capability))
        {
            let reason = format!(
                "target {} lacks {}",
                target_caps.version,
                capability.description()
            );
            partition.target_gap(policy, suite, case, GapKind::TargetCapability, reason);
        } else {
            partition.runnable.push(case);
        }
    }
    Ok(partition)
}

/// Capabilities a case needs. Pulls from two sources, in order of priority:
///
///   1. Legacy hardcoded table for pinned-manifest cases that don't carry
///      capability tokens (ids 92, 134, 154, 155, 156, 222).
///   2. The `required_capabilities` field on the case itself, populated by
///      new shards under `corpus/sqlite_parity/cases/`. A token this runner
///      does not know is an error: dropping it would run the case ungated.
pub fn required_capabilities(case: &Case) -> Result<Vec<Capability>> {
    let mut caps = match case.id {
        92 => vec![Capability::PercentileFunctions],
        93 | 94 => vec![Capability::Fts5],
        95 => vec![Capability::Rtree],
        96 => vec![Capability::Dbstat],
        134 => vec![Capability::DotCrlf],
        154 => vec![Capability::DotDbInfo],
        155 => vec![Capability::DotDbTotxt],
        156 => vec![Capability::DotRecover],
        222 => vec![Capability::EscapeSymbolOption],
        _ => Vec::new(),
    };
    for token in &case.required_capabilities {
        let Some(cap) = Capability::from_token(token) else {
            bail!(
                "case {} {} requires unknown capability token {token:?}",
                case.display_id(),
                case.name
            );
        };
        if !caps.contains(&cap) {
            caps.push(cap);
        }
    }
    Ok(caps)
}

/// Every capability probe, as (capability, how to probe it). A probe that
/// runs and fails means the capability is absent; one that cannot run,
/// times out or floods is an error (SQ-05): it never becomes a skip.
fn probe_capabilities(bin: &Path, limits: Limits) -> Result<ShellCapabilities> {
    let version = probe_version(bin)?;
    let probe = |script: &str| run_sql_script(bin, script, limits);
    let help_contains = |needle: &str| shell_help_contains(bin, needle, limits);
    Ok(ShellCapabilities {
        version,
        percentile_functions: probe(
            ".mode list\n.headers off\nCREATE TABLE t(x INTEGER); INSERT INTO t VALUES (1), (2), (3);\nSELECT median(x), percentile_cont(x,0.5) FROM t;\n",
        )?,
        dot_crlf: help_contains(".crlf")?,
        dot_dbinfo: help_contains(".dbinfo")?,
        dot_dbtotxt: help_contains(".dbtotxt")?,
        dot_recover: help_contains(".recover")?,
        escape_symbol_option: escape_symbol_option_supported(bin, limits)?,
        fts5: probe("CREATE VIRTUAL TABLE _probe_fts USING fts5(x);\n")?,
        rtree: probe("CREATE VIRTUAL TABLE _probe_rtree USING rtree(id, x0, x1, y0, y1);\n")?,
        dbstat: probe(
            "CREATE TABLE _probe_t(x);\nINSERT INTO _probe_t VALUES(1);\nCREATE VIRTUAL TABLE main._probe_stat USING dbstat;\nSELECT count(*) FROM _probe_stat;\n",
        )?,
        jsonb: probe("SELECT length(jsonb('1'));\n")?,
        math1: probe("SELECT round(acos(1.0),3), round(sqrt(4.0),3);\n")?,
        generate_series: probe("SELECT count(*) FROM generate_series(1,3);\n")?,
        json_pretty: probe("SELECT json_pretty('{\"a\":1}');\n")?,
        jsonb_array_insert: probe("SELECT length(jsonb_array_insert('[]', '$[0]', 1));\n")?,
        regexp: probe("SELECT 'abc' REGEXP 'b';\n")?,
    })
}

impl EngineSpec {
    pub fn new(name: impl Into<String>, bin: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            bin: bin.into(),
            limits: Limits::default(),
            identity: Arc::new(OnceLock::new()),
        }
    }

    pub fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// `run_case`, with a run that could not be prepared, spawned or waited
    /// for turned into an output whose outcome says so (`spawn_error`): an
    /// engine error fails its case, never the whole run.
    pub fn run_case_bounded(
        &self,
        case: &Case,
        tmp_root: &Path,
        memory_samples: bool,
    ) -> EngineOutput {
        let started = Instant::now();
        self.run_case(case, tmp_root, memory_samples)
            .unwrap_or_else(|error| {
                self.not_started(started.elapsed(), memory_samples, format!("{error:#}"))
            })
    }

    fn not_started(&self, elapsed: Duration, memory_samples: bool, error: String) -> EngineOutput {
        let identity = self.binary_identity().ok();
        EngineOutput {
            engine: self.name.clone(),
            executable_path: identity.as_ref().map_or_else(
                || self.bin.display().to_string(),
                |id| id.executable_path.clone(),
            ),
            executable_sha256: identity
                .as_ref()
                .map(|id| id.executable_sha256.clone())
                .unwrap_or_default(),
            version: identity.map(|id| id.version).unwrap_or_default(),
            status_code: None,
            elapsed,
            stdout: Vec::new(),
            stderr: Vec::new(),
            memory_status: ProcessMemory::default().status(memory_samples).to_owned(),
            peak_rss_kb: None,
            rss_sampled_kb: None,
            outcome: ExecutionOutcome::SpawnError,
            failure: Some(format!("could not run: {error}")),
        }
    }

    /// The output of one bounded run.
    fn output(
        &self,
        identity: BinaryIdentity,
        captured: Captured,
        memory_samples: bool,
    ) -> EngineOutput {
        let failure = match captured.outcome {
            ExecutionOutcome::Timeout => Some(format!(
                "timed out after {} ms; its process group was killed",
                self.limits.timeout_ms()
            )),
            ExecutionOutcome::OutputLimit => Some(format!(
                "wrote more than {} bytes to stdout or stderr; its process group was killed",
                self.limits.max_output_bytes
            )),
            _ => None,
        };
        EngineOutput {
            engine: self.name.clone(),
            executable_path: identity.executable_path,
            executable_sha256: identity.executable_sha256,
            version: identity.version,
            status_code: captured.status.code(),
            elapsed: captured.elapsed,
            stdout: captured.stdout,
            stderr: captured.stderr,
            memory_status: captured.memory.status(memory_samples).to_owned(),
            peak_rss_kb: captured.memory.peak_rss_kb,
            rss_sampled_kb: captured.memory.rss_sampled_kb,
            outcome: captured.outcome,
            failure,
        }
    }

    pub fn run_case(
        &self,
        case: &Case,
        tmp_root: &Path,
        memory_samples: bool,
    ) -> Result<EngineOutput> {
        let identity = self.binary_identity()?;
        let case_tmp = tmp_root.join(format!(
            "{}-{}-{}",
            case.display_id(),
            sanitize_identifier(&self.name),
            std::process::id()
        ));
        if case_tmp.exists() {
            make_removable(&case_tmp).with_context(|| {
                format!(
                    "prepare previous sqlite parity tmpdir {} for removal",
                    case_tmp.display()
                )
            })?;
            fs::remove_dir_all(&case_tmp).with_context(|| {
                format!(
                    "remove previous sqlite parity tmpdir {}",
                    case_tmp.display()
                )
            })?;
        }
        fs::create_dir_all(&case_tmp)
            .with_context(|| format!("create sqlite parity tmpdir {}", case_tmp.display()))?;
        for (name, contents) in &case.files {
            let path = case_tmp.join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create fixture parent {}", parent.display()))?;
            }
            fs::write(&path, replace_tmp(contents, &case_tmp))
                .with_context(|| format!("write fixture {}", path.display()))?;
        }

        if let Some(script) = &case.script {
            return self.run_script(case, script, &case_tmp, identity, memory_samples);
        }

        let db_path = db_path_for(&self.name, case, tmp_root, &case_tmp)?;
        let mut command = Command::new(&self.bin);
        if case.args.is_empty() {
            if is_sqlite_shell(&self.name) {
                command.arg("-batch").arg("-bail").arg(&db_path);
            } else {
                command.arg("--batch").arg("--bail").arg(&db_path);
            }
        } else {
            for arg in &case.args {
                command.arg(replace_tmp(arg, &case_tmp));
            }
        }
        let captured = run_command(
            &mut command,
            Some(replace_tmp(&case.stdin, &case_tmp)),
            &case_tmp,
            &self.name,
            memory_samples,
            self.limits,
        )
        .with_context(|| format!("run {} case {}", self.name, case.display_id()))?;
        Ok(self.output(identity, captured, memory_samples))
    }

    /// The capabilities of this sqlite-shell-compatible CLI (the sqlite3
    /// reference or the redlinedb target), each probed under the engine's
    /// limits. A probe that cannot run is an error, never a missing
    /// capability.
    pub fn capabilities(&self) -> Result<ShellCapabilities> {
        probe_capabilities(&self.bin, self.limits)
            .with_context(|| format!("probe {} capabilities of {}", self.name, self.bin.display()))
    }

    pub fn binary_identity(&self) -> Result<BinaryIdentity> {
        match self
            .identity
            .get_or_init(|| binary_identity(&self.bin).map_err(|err| err.to_string()))
        {
            Ok(identity) => Ok(identity.clone()),
            Err(message) => bail!("{message}"),
        }
    }

    fn run_script(
        &self,
        case: &Case,
        script: &str,
        case_tmp: &Path,
        identity: BinaryIdentity,
        memory_samples: bool,
    ) -> Result<EngineOutput> {
        let script_path = case_tmp.join("case.sh");
        fs::write(&script_path, replace_tmp(script, case_tmp))
            .with_context(|| format!("write script {}", script_path.display()))?;
        let mut command = Command::new("bash");
        command
            .arg(&script_path)
            .env("SQLITE_BIN", &self.bin)
            .env("SQLITE_PARITY_TMP", case_tmp);
        let captured = run_command(
            &mut command,
            None,
            case_tmp,
            &self.name,
            memory_samples,
            self.limits,
        )
        .with_context(|| format!("run script case {}", case.display_id()))?;
        Ok(self.output(identity, captured, memory_samples))
    }
}

/// One bounded run (`bounded`): piped when memory is not sampled, else with
/// stdout and stderr in files beside the case so the child can be polled.
fn run_command(
    command: &mut Command,
    stdin_text: Option<String>,
    case_tmp: &Path,
    engine_name: &str,
    memory_samples: bool,
    limits: Limits,
) -> Result<Captured> {
    let stdin = stdin_text.map(String::into_bytes);
    if !memory_samples {
        return bounded::run_piped(command, stdin, limits);
    }
    let output_prefix = sanitize_identifier(engine_name);
    bounded::run_to_files(
        command,
        stdin,
        limits,
        &case_tmp.join(format!("{output_prefix}.stdout")),
        &case_tmp.join(format!("{output_prefix}.stderr")),
        memory_samples,
    )
}

pub fn binary_identity(bin: &Path) -> Result<BinaryIdentity> {
    let path = resolve_executable_path(bin)?;
    let bytes = fs::read(&path).with_context(|| format!("read executable {}", path.display()))?;
    let executable_sha256 = format!("{:x}", Sha256::digest(&bytes));
    Ok(BinaryIdentity {
        executable_path: path.to_string_lossy().into_owned(),
        executable_sha256,
        version: probe_version(&path)?,
    })
}

pub fn resolve_executable_path(path: &Path) -> Result<PathBuf> {
    if path.components().count() > 1 || path.is_absolute() {
        return fs::canonicalize(path)
            .with_context(|| format!("canonicalize executable {}", path.display()));
    }
    let Some(path_var) = std::env::var_os("PATH") else {
        bail!("PATH is unset while resolving {}", path.display());
    };
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(path);
        if candidate.is_file() {
            return fs::canonicalize(&candidate)
                .with_context(|| format!("canonicalize executable {}", candidate.display()));
        }
    }
    bail!("executable not found on PATH: {}", path.display())
}

fn db_path_for(engine: &str, case: &Case, tmp_root: &Path, case_tmp: &Path) -> Result<String> {
    let db = replace_tmp(&case.db, case_tmp);
    if db != ":memory:" {
        return Ok(db);
    }
    match case.profile {
        Profile::Tempfile => {
            fs::create_dir_all(tmp_root)
                .with_context(|| format!("create sqlite parity tmpdir {}", tmp_root.display()))?;
            let path = case_tmp.join(format!("{}.db", sanitize_identifier(engine)));
            path.to_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("non-utf8 sqlite parity db path {}", path.display()))
        }
        _ => Ok(":memory:".to_owned()),
    }
}

pub(crate) fn is_sqlite_shell(engine_name: &str) -> bool {
    engine_name.eq_ignore_ascii_case("sqlite3") || engine_name.eq_ignore_ascii_case("sqlite")
}

fn replace_tmp(input: &str, tmp: &Path) -> String {
    input.replace("{{TMP}}", &tmp.to_string_lossy())
}

/// `<bin> --version`, bounded like a case run.
fn probe_version(bin: &Path) -> Result<String> {
    let output = bounded::run_piped(Command::new(bin).arg("--version"), None, Limits::default())
        .with_context(|| format!("run {} --version", bin.display()))?;
    if !output.outcome.is_complete() {
        bail!(
            "{} --version ended with {}",
            bin.display(),
            output.outcome.as_str()
        );
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if version.is_empty() {
        Ok(String::from("<unknown>"))
    } else {
        Ok(version)
    }
}

/// Runs a probe script on `:memory:` under `limits`: whether it succeeded,
/// or an error when it could not give an answer.
fn run_probe(
    bin: &Path,
    script: &str,
    extra_args: &[&str],
    limits: Limits,
) -> Result<bounded::Captured> {
    let mut command = Command::new(bin);
    command.arg("-batch").arg("-bail");
    for arg in extra_args {
        command.arg(arg);
    }
    command.arg(":memory:");
    let captured = bounded::run_piped(&mut command, Some(script.as_bytes().to_vec()), limits)
        .with_context(|| format!("probe {} with {script:?}", bin.display()))?;
    if !captured.outcome.is_complete() {
        bail!(
            "capability probe {script:?} of {} ended with {}",
            bin.display(),
            captured.outcome.as_str()
        );
    }
    Ok(captured)
}

fn run_sql_script(bin: &Path, script: &str, limits: Limits) -> Result<bool> {
    Ok(run_probe(bin, script, &[], limits)?.status.success())
}

fn shell_help_contains(bin: &Path, needle: &str, limits: Limits) -> Result<bool> {
    let output = run_probe(bin, ".help\n.quit\n", &[], limits)?;
    Ok(output.status.success() && String::from_utf8_lossy(&output.stdout).contains(needle))
}

fn escape_symbol_option_supported(bin: &Path, limits: Limits) -> Result<bool> {
    Ok(
        run_probe(bin, "SELECT char(1);\n", &["-escape", "symbol"], limits)?
            .status
            .success(),
    )
}

#[cfg(unix)]
fn make_removable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("stat {}", path.display()))?;
    let mode = if metadata.is_dir() { 0o700 } else { 0o600 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("chmod {}", path.display()))?;
    if metadata.is_dir() {
        for entry in fs::read_dir(path).with_context(|| format!("read {}", path.display()))? {
            let entry = entry.with_context(|| format!("read entry in {}", path.display()))?;
            make_removable(&entry.path())?;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn make_removable(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::bounded::{Limits, read_capped};
    use super::run_command;
    use std::fs;
    use std::process::Command;

    #[test]
    fn captured_binary_output_is_preserved_byte_for_byte() {
        let path = std::env::temp_dir().join(format!(
            "redline-testing-binary-output-{}",
            std::process::id()
        ));
        fs::write(&path, [b'a', 0xff, b'b']).expect("write binary output fixture");
        let captured = read_capped(&path, 1024).expect("read binary output");
        fs::remove_file(path).expect("remove binary output fixture");
        assert_eq!(captured, b"a\xffb");
    }

    #[test]
    fn both_capture_paths_keep_invalid_utf8() {
        // The piped path (sqlite_parity) and the file path (memory suite)
        // must hand the comparison the same raw bytes.
        let case_tmp = std::env::temp_dir().join(format!(
            "redline-testing-capture-paths-{}",
            std::process::id()
        ));
        fs::create_dir_all(&case_tmp).expect("create capture tmp");
        for memory_samples in [false, true] {
            let mut command = Command::new("sh");
            command
                .arg("-c")
                .arg("printf 'a\\253\\r\\nb \\n'; printf 'e\\377' >&2");
            let captured = run_command(
                &mut command,
                None,
                &case_tmp,
                "probe",
                memory_samples,
                Limits::default(),
            )
            .expect("run capture probe");
            assert_eq!(
                captured.stdout, b"a\xab\r\nb \n",
                "memory_samples={memory_samples}"
            );
            assert_eq!(captured.stderr, b"e\xff", "memory_samples={memory_samples}");
        }
        fs::remove_dir_all(case_tmp).expect("remove capture tmp");
    }
}
