//! `modpack test`: checks the conditions of a mode, or of all modes.
//!
//! Each mode gets its resolved conditions (global ones, then its own), or only
//! the one `--condition` names. Every condition passes, fails or is skipped,
//! with the reason; the command fails when one condition fails.

use std::io::Write;
use std::path::Path;

use serde_json::json;

use crate::conditions::{self, Checker, Status, server::Machine};
use crate::context::Context;
use crate::error::{Error, Result};
use crate::model::manifest::{self, Manifest};
use crate::model::modes::{self, Modes};
use crate::store::download::Http;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Mode to check; all modes when omitted
    pub mode: Option<String>,
    /// Check only this condition
    #[arg(long)]
    pub condition: Option<String>,
    /// Print the report as JSON
    #[arg(long)]
    pub json: bool,
}

pub fn run(args: Args, ctx: &Context) -> Result<()> {
    let root = ctx.require_root()?;
    let http = Http::new();
    let machine = Machine::from_env(&http)?;
    test(root, &args, &machine, &mut std::io::stdout().lock())
}

/// One mode and what its conditions gave.
struct ModeReport {
    name: String,
    results: Vec<(String, String, Status)>,
}

/// The command, with the machine given.
fn test(root: &Path, args: &Args, machine: &Machine<'_>, out: &mut dyn Write) -> Result<()> {
    if let Some(name) = &args.condition
        && !conditions::NAMES.contains(&name.as_str())
    {
        return Err(Error::usage(
            format!("unknown condition `{name}`"),
            format!("use one of: {}", conditions::NAMES.join(", ")),
        ));
    }
    let manifest = load_manifest(root)?;
    let modes_path = root.join(modes::FILE_NAME);
    if !modes_path.is_file() {
        return Err(Error::usage(
            format!("no {} in this repository", modes::FILE_NAME),
            format!(
                "create {} at the repository root, with at least a [default] table",
                modes::FILE_NAME
            ),
        ));
    }
    let modes = Modes::load(&modes_path)?;
    let names: Vec<String> = match &args.mode {
        Some(mode) => vec![mode.clone()],
        None => modes.names().into_iter().map(str::to_owned).collect(),
    };
    // Resolve every mode first: a wrong mode name fails before any check.
    let resolved = names
        .iter()
        .map(|name| modes.resolve(name, &manifest))
        .collect::<Result<Vec<_>>>()?;

    let checker = Checker {
        root,
        manifest: &manifest,
        machine,
    };
    let mut reports = Vec::new();
    for mode in &resolved {
        if !args.json {
            writeln!(out, "{}", mode.name)?;
        }
        let mut report = ModeReport {
            name: mode.name.clone(),
            results: Vec::new(),
        };
        let selected = mode
            .conditions
            .iter()
            .filter(|c| args.condition.as_deref().is_none_or(|n| c.name() == n));
        for condition in selected {
            let status = checker.check(condition, mode);
            let label = conditions::label(condition);
            if !args.json {
                write_result(out, &label, &status)?;
            }
            report
                .results
                .push((condition.name().to_owned(), label, status));
        }
        if !args.json && report.results.is_empty() {
            match &args.condition {
                Some(name) => writeln!(out, "  no condition `{name}` in this mode")?,
                None => writeln!(out, "  no conditions")?,
            }
        }
        reports.push(report);
    }

    let count = |status: &str| {
        reports
            .iter()
            .flat_map(|r| &r.results)
            .filter(|(_, _, s)| s.name() == status)
            .count()
    };
    let (passed, failed, skipped) = (count("passed"), count("failed"), count("skipped"));
    if args.json {
        writeln!(out, "{}", to_json(&reports, passed, failed, skipped))?;
    } else {
        writeln!(out, "{passed} passed, {failed} failed, {skipped} skipped")?;
    }
    if failed > 0 {
        return Err(Error::internal(
            format!("{failed} condition(s) failed"),
            "see the reasons in the report, fix the pack, then run `modpack test` again",
        ));
    }
    Ok(())
}

fn load_manifest(root: &Path) -> Result<Manifest> {
    let path = root.join(manifest::FILE_NAME);
    if !path.is_file() {
        return Err(Error::usage(
            format!("no {} in this repository", manifest::FILE_NAME),
            "generate it with `modpack manifest`",
        ));
    }
    Manifest::load(&path)
}

/// `  failed   side = "server": reason`, the reason's further lines indented.
fn write_result(out: &mut dyn Write, label: &str, status: &Status) -> Result<()> {
    write!(out, "  {:<8} {label}", status.name())?;
    match status.reason() {
        None => writeln!(out)?,
        Some(reason) => {
            let mut lines = reason.lines();
            writeln!(out, ": {}", lines.next().unwrap_or_default())?;
            for line in lines {
                writeln!(out, "      {line}")?;
            }
        }
    }
    Ok(())
}

fn to_json(reports: &[ModeReport], passed: usize, failed: usize, skipped: usize) -> String {
    let modes: Vec<_> = reports
        .iter()
        .map(|r| {
            let conditions: Vec<_> = r
                .results
                .iter()
                .map(|(name, label, status)| {
                    json!({
                        "condition": name,
                        "label": label,
                        "status": status.name(),
                        "reason": status.reason(),
                    })
                })
                .collect();
            json!({ "mode": r.name, "conditions": conditions })
        })
        .collect();
    // Pretty, like the JSON of `status` and `pack`.
    serde_json::to_string_pretty(&json!({
        "modes": modes,
        "passed": passed,
        "failed": failed,
        "skipped": skipped,
    }))
    .expect("a JSON value always serializes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use crate::store::download::tests::NoFetch;
    use std::fs;
    use std::path::PathBuf;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repo")
    }

    fn machine() -> Machine<'static> {
        Machine {
            fetch: &NoFetch,
            java: PathBuf::from("/nonexistent/modpack-test/bin/java"),
            cache_dir: PathBuf::from("/nonexistent/modpack-test/cache"),
        }
    }

    fn args(mode: Option<&str>, condition: Option<&str>, json: bool) -> Args {
        Args {
            mode: mode.map(str::to_owned),
            condition: condition.map(str::to_owned),
            json,
        }
    }

    fn run_on(root: &Path, args: Args) -> (Result<()>, String) {
        let mut out = Vec::new();
        let result = test(root, &args, &machine(), &mut out);
        (result, String::from_utf8(out).unwrap())
    }

    /// The fixture repository with another `modes.toml`.
    fn repo_with_modes(modes: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::copy(
            fixture().join(manifest::FILE_NAME),
            dir.path().join(manifest::FILE_NAME),
        )
        .unwrap();
        fs::write(dir.path().join(modes::FILE_NAME), modes).unwrap();
        dir
    }

    #[test]
    fn static_conditions_of_every_mode_pass_on_the_fixture() {
        let (result, out) = run_on(&fixture(), args(None, Some("deps-complete"), false));
        result.unwrap();
        assert_eq!(
            out,
            "default\n  passed   deps-complete\nhardcore\n  passed   deps-complete\n\
             server\n  passed   deps-complete\n3 passed, 0 failed, 0 skipped\n"
        );
        let (result, out) = run_on(&fixture(), args(Some("server"), Some("side"), false));
        result.unwrap();
        assert_eq!(
            out,
            "server\n  passed   side = \"server\"\n1 passed, 0 failed, 0 skipped\n"
        );
    }

    #[test]
    fn a_failing_global_condition_fails_the_command() {
        let repo = repo_with_modes(
            "[conditions]\ndeps-complete = true\nclient-starts = true\n\
             [modes.lowend]\nremove = [\"sodium\"]\n",
        );
        let (result, out) = run_on(repo.path(), args(Some("lowend"), None, false));
        let err = result.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Internal);
        assert!(err.to_string().contains("1 condition(s) failed"));
        assert!(
            out.contains(
                "  failed   deps-complete: `iris` needs `sodium`, which is not in this mode\n"
            ),
            "{out}"
        );
        assert!(
            out.contains("  skipped  client-starts: client-starts is not implemented"),
            "{out}"
        );
        assert!(out.ends_with("0 passed, 1 failed, 1 skipped\n"), "{out}");
    }

    #[test]
    fn json_report_holds_every_result() {
        let repo = repo_with_modes(
            "[conditions]\ndeps-complete = true\n[default.conditions]\nside = \"server\"\n",
        );
        let (result, out) = run_on(repo.path(), args(None, None, true));
        assert!(result.is_err());
        let report: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(report["passed"], 1);
        assert_eq!(report["failed"], 1);
        let conditions = &report["modes"][0]["conditions"];
        assert_eq!(report["modes"][0]["mode"], "default");
        assert_eq!(conditions[0]["condition"], "deps-complete");
        assert_eq!(conditions[0]["status"], "passed");
        assert!(conditions[0]["reason"].is_null());
        assert_eq!(conditions[1]["label"], "side = \"server\"");
        assert_eq!(conditions[1]["status"], "failed");
        assert!(
            conditions[1]["reason"]
                .as_str()
                .unwrap()
                .contains("iris, sodium")
        );
    }

    #[test]
    fn server_starts_on_an_unhandled_loader_is_skipped() {
        let repo = repo_with_modes("[conditions]\nserver-starts = true\n");
        let text = fs::read_to_string(repo.path().join(manifest::FILE_NAME)).unwrap();
        fs::write(
            repo.path().join(manifest::FILE_NAME),
            text.replace("loader = \"neoforge\"", "loader = \"forge\""),
        )
        .unwrap();
        let (result, out) = run_on(repo.path(), args(None, None, false));
        result.unwrap();
        assert!(
            out.contains("  skipped  server-starts (timeout 300s): no scriptable server install for the loader `forge`"),
            "{out}"
        );
    }

    #[test]
    fn mode_without_the_asked_condition_says_so() {
        let (result, out) = run_on(&fixture(), args(Some("hardcore"), Some("side"), false));
        result.unwrap();
        assert_eq!(
            out,
            "hardcore\n  no condition `side` in this mode\n0 passed, 0 failed, 0 skipped\n"
        );
    }

    #[test]
    fn unknown_mode_or_condition_is_a_usage_error() {
        let (result, out) = run_on(&fixture(), args(Some("lowend"), None, false));
        assert_eq!(result.unwrap_err().kind(), ErrorKind::Usage);
        assert_eq!(out, "", "nothing checked");
        let err = run_on(&fixture(), args(None, Some("fast"), false))
            .0
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
        assert!(
            err.hint()
                .contains("side, deps-complete, server-starts, client-starts")
        );
    }

    #[test]
    fn missing_modes_file_is_a_usage_error() {
        let repo = repo_with_modes("");
        fs::remove_file(repo.path().join(modes::FILE_NAME)).unwrap();
        let err = run_on(repo.path(), args(None, None, false)).0.unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Usage);
        assert!(err.to_string().contains("no modes.toml"));
    }
}
