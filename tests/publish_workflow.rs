//! The publish workflow the socle lays down, and the reusable workflow it calls.

use std::fs;
use std::path::Path;
use std::process::Command as Process;

use assert_cmd::Command;
use modpack_cli::model::socle::{SocleConfig, SocleLock};

const CALLER: &str = ".github/workflows/modpack-publish.yml";

fn modpack(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("modpack").unwrap();
    cmd.current_dir(dir).env_remove("RUST_LOG");
    cmd
}

fn init_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    modpack(tmp.path())
        .args([
            "init",
            "pack",
            "--name=Test Pack",
            "--mc-version=1.21.1",
            "--loader=neoforge",
            "--loader-version=21.1.77",
            "--author=Tester",
        ])
        .assert()
        .success();
    let root = tmp.path().join("pack");
    (tmp, root)
}

fn template() -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("templates/socle")
            .join(CALLER),
    )
    .unwrap()
}

#[test]
fn init_lays_down_the_publish_workflow() {
    let (_tmp, root) = init_repo();
    assert_eq!(fs::read_to_string(root.join(CALLER)).unwrap(), template());
    assert!(
        SocleLock::load(&root)
            .unwrap()
            .files
            .iter()
            .any(|f| f.path == CALLER)
    );
}

#[test]
fn update_lays_down_the_publish_workflow_in_an_older_repository() {
    let (_tmp, root) = init_repo();
    // A repository from before the workflow existed: neither the file nor
    // its registry entry.
    fs::remove_file(root.join(CALLER)).unwrap();
    let mut lock = SocleLock::load(&root).unwrap();
    lock.files.retain(|f| f.path != CALLER);
    lock.save(&root).unwrap();
    let mut config = SocleConfig::load(&root).unwrap();
    config.version = "0.0.1".into();
    config.save(&root).unwrap();

    modpack(&root).arg("update").assert().success();

    assert_eq!(fs::read_to_string(root.join(CALLER)).unwrap(), template());
    assert!(
        SocleLock::load(&root)
            .unwrap()
            .files
            .iter()
            .any(|f| f.path == CALLER)
    );
}

/// The caller pins the reusable workflow and the CLI to the release that
/// embeds it, so that `modpack update` moves both together.
#[test]
fn the_caller_pins_this_release() {
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    let text = template();
    assert!(
        text.contains(&format!(
            "uses: payangar-studio/modpack-cli/.github/workflows/publish-modpack.yml@{tag}\n"
        )),
        "the reusable workflow is not pinned to {tag}"
    );
    assert!(
        text.contains(&format!("modpack-version: {tag}\n")),
        "the CLI is not pinned to {tag}"
    );
}

#[test]
fn the_reusable_workflow_builds_the_packs_without_the_jars() {
    let workflow = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows/publish-modpack.yml"),
    )
    .unwrap();
    assert!(workflow.contains("modpack pack "));
    for step in ["modpack install", "modpack manifest"] {
        assert!(!workflow.contains(step), "the workflow runs `{step}`");
    }
}

fn reusable() -> String {
    fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows/publish-modpack.yml"),
    )
    .unwrap()
}

/// The `run` script of the step `name` of the reusable workflow.
fn step_script(name: &str) -> String {
    let workflow = reusable();
    let mut lines = workflow
        .lines()
        .skip_while(|l| l.trim() != format!("- name: {name}"))
        .skip_while(|l| l.trim() != "run: |")
        .skip(1);
    let indent = " ".repeat(10);
    let mut script = String::new();
    for line in lines.by_ref() {
        if !line.is_empty() && !line.starts_with(&indent) {
            break;
        }
        script.push_str(line.strip_prefix(&indent).unwrap_or(line));
        script.push('\n');
    }
    assert!(!script.is_empty(), "no step `{name}`");
    script
}

/// What the step `Find the projects` leaves: exit success, the variables it
/// sets, and the run summary.
fn find_projects(page_toml: &str, platforms: &str, dry_run: bool) -> (bool, String, String) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("page")).unwrap();
    fs::write(dir.path().join("page/page.toml"), page_toml).unwrap();
    let env = dir.path().join("env");
    let summary = dir.path().join("summary");
    let status = Process::new("bash")
        .args(["-e", "-c", &step_script("Find the projects")])
        .current_dir(dir.path())
        .env("GITHUB_ENV", &env)
        .env("GITHUB_STEP_SUMMARY", &summary)
        .env("PLATFORMS", platforms)
        .env("DRY_RUN", dry_run.to_string())
        .env("MODRINTH_INPUT", "")
        .env("CURSEFORGE_INPUT", "")
        .status()
        .unwrap();
    let read = |p: &Path| fs::read_to_string(p).unwrap_or_default();
    (status.success(), read(&env), read(&summary))
}

const MODRINTH_ONLY: &str = "[modrinth]\nproject_id = \"AbC\"\n\n[curseforge]\nproject_id = \"\"\n";

#[test]
fn a_platform_without_project_is_skipped_with_a_notice() {
    let (ok, env, summary) = find_projects(MODRINTH_ONLY, "modrinth curseforge", false);
    assert!(ok);
    assert!(env.contains("MODRINTH_PROJECT=AbC\n"), "{env}");
    assert!(env.contains("PUBLISH_PLATFORMS=modrinth\n"), "{env}");
    assert!(summary.contains("curseforge skipped"), "{summary}");
    assert!(summary.contains("modpack-curseforge-page"), "{summary}");

    // A dry run builds every platform asked.
    let (ok, env, _) = find_projects(MODRINTH_ONLY, "modrinth curseforge", true);
    assert!(ok);
    assert!(
        env.contains("PUBLISH_PLATFORMS=modrinth curseforge\n"),
        "{env}"
    );

    // No platform left: nothing would be published, the run fails.
    let (ok, _, _) = find_projects(MODRINTH_ONLY, "curseforge", false);
    assert!(!ok);
}

#[test]
fn only_the_platforms_with_a_project_are_built_and_published() {
    let workflow = reusable();
    assert!(step_script("Build the packs").contains("<<<\"$PUBLISH_PLATFORMS\""));
    for platform in ["modrinth", "curseforge"] {
        assert!(
            workflow.contains(&format!("contains(env.PUBLISH_PLATFORMS, '{platform}')")),
            "{platform}"
        );
    }
    assert!(!workflow.contains("contains(inputs.platforms"));
}

/// The jq filter the publish steps read the artifacts of a platform with.
fn artifacts_filter() -> String {
    let workflow = reusable();
    let line = workflow
        .lines()
        .find_map(|l| l.trim().strip_prefix("ARTIFACTS: "))
        .expect("ARTIFACTS is set");
    line.trim_matches('\'').to_owned()
}

#[test]
fn the_default_variant_is_published_last_on_each_platform() {
    for (step, platform) in [
        ("Publish to Modrinth", "modrinth"),
        ("Publish to CurseForge", "curseforge"),
    ] {
        let script = step_script(step);
        assert!(
            script.contains(&format!("jq -r --arg platform {platform} \"$ARTIFACTS\"")),
            "{step} does not read its artifacts through $ARTIFACTS"
        );
        assert!(
            script.contains("the default variant was not published"),
            "{step}"
        );
    }

    let report = r#"{"artifacts": [
        {"variant": "default", "platform": "modrinth", "path": "d.mrpack"},
        {"variant": "server", "platform": "modrinth", "path": "s.mrpack"},
        {"variant": "default", "platform": "curseforge", "path": "d.zip"},
        {"variant": "lite", "platform": "modrinth", "path": "l.mrpack"}
    ]}"#;
    let output = Process::new("jq")
        .args(["-r", "--arg", "platform", "modrinth", &artifacts_filter()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut jq| {
            use std::io::Write;
            jq.stdin.take().unwrap().write_all(report.as_bytes())?;
            jq.wait_with_output()
        })
        .expect("jq runs");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "server\ts.mrpack\nlite\tl.mrpack\ndefault\td.mrpack\n"
    );
}

/// Runs the step `Check that the local mods are approved` in `root`, with the
/// `modpack` under test in `PATH`: exit success, output, run summary.
fn approval_gate(root: &Path, dry_run: bool) -> (bool, String, String) {
    let bin = Path::new(env!("CARGO_BIN_EXE_modpack")).parent().unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin.to_owned())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let summary = temp.path().join("summary");
    let out = Process::new("bash")
        .args([
            "-e",
            "-c",
            &step_script("Check that the local mods are approved"),
        ])
        .current_dir(root)
        .env("PATH", path)
        .env("RUNNER_TEMP", temp.path())
        .env("GITHUB_STEP_SUMMARY", &summary)
        .env("DRY_RUN", dry_run.to_string())
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        fs::read_to_string(&summary).unwrap_or_default(),
    )
}

#[test]
fn the_publish_gate_refuses_an_unapproved_local_mod() {
    let (_tmp, root) = init_repo();
    let (ok, out, _) = approval_gate(&root, false);
    assert!(ok, "no local mod: {out}");

    fs::create_dir_all(root.join("mods-local")).unwrap();
    fs::write(root.join("mods-local/custom-tweaks-0.3.jar"), b"jar").unwrap();
    // A local jar only: the manifest asks no platform, offline or not.
    modpack(&root).arg("manifest").assert().success();

    let (ok, out, summary) = approval_gate(&root, false);
    assert!(!ok, "{out}");
    assert!(
        out.contains("::error::`custom-tweaks` (mods-local/custom-tweaks-0.3.jar) is not approved"),
        "{out}"
    );
    assert!(out.contains("`modpack approve custom-tweaks`"), "{out}");
    assert!(summary.contains("Not published"), "{summary}");

    // A dry run publishes nothing: it warns.
    let (ok, out, _) = approval_gate(&root, true);
    assert!(ok, "{out}");
    assert!(out.contains("::warning::`custom-tweaks`"), "{out}");

    modpack(&root)
        .args(["approve", "custom-tweaks"])
        .assert()
        .success();
    let (ok, out, _) = approval_gate(&root, false);
    assert!(ok, "{out}");
    assert!(out.contains("all approved"), "{out}");

    // A new jar under the same name loses the approval.
    fs::write(
        root.join("mods-local/custom-tweaks-0.3.jar"),
        b"jar, rebuilt",
    )
    .unwrap();
    modpack(&root).arg("manifest").assert().success();
    let (ok, out, _) = approval_gate(&root, false);
    assert!(!ok, "{out}");
}

#[test]
fn the_build_step_merges_the_reports_and_their_warnings() {
    let script = step_script("Build the packs");
    let filter = script
        .split("jq -s '")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .expect("the reports are merged with jq -s");
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.json");
    let b = dir.path().join("b.json");
    fs::write(
        &a,
        r#"{"artifacts": [{"variant": "default"}], "blocked": [], "warnings": ["w1", "w2"]}"#,
    )
    .unwrap();
    fs::write(
        &b,
        r#"{"artifacts": [{"variant": "server"}], "blocked": [], "warnings": ["w1"]}"#,
    )
    .unwrap();
    let out = Process::new("jq")
        .args(["-c", "-s", filter])
        .arg(&a)
        .arg(&b)
        .output()
        .expect("jq runs");
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "{\"artifacts\":[{\"variant\":\"default\"},{\"variant\":\"server\"}],\"blocked\":[],\"warnings\":[\"w1\",\"w2\"]}\n"
    );
    assert!(script.contains(r#"jq -r '.warnings[] | "::warning::\(.)"'"#));
}
