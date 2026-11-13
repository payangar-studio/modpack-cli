//! The AI layer the socle lays down: the `modpack-return` and `modpack-audit`
//! skills and the instructions block of `AGENTS.md`.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use clap::CommandFactory;
use modpack_cli::cli::Cli;
use modpack_cli::model::socle::{SocleConfig, SocleLock};

const SKILLS: [&str; 2] = ["modpack-return", "modpack-audit"];
const BLOCK: &str = "templates/blocks/agents.md";

fn skill_path(name: &str) -> String {
    format!(".claude/skills/{name}/SKILL.md")
}

fn template(relative: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)).unwrap()
}

fn skill_template(name: &str) -> String {
    template(&format!("templates/socle/{}", skill_path(name)))
}

fn modpack(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("modpack").unwrap();
    cmd.current_dir(dir).env_remove("RUST_LOG");
    cmd
}

fn init_repo() -> (tempfile::TempDir, PathBuf) {
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

/// The text between the socle's markers in `AGENTS.md`.
fn agents_block(root: &Path) -> String {
    let text = fs::read_to_string(root.join("AGENTS.md")).unwrap();
    let start = text.find("<!-- modpack:begin -->\n").unwrap() + 23;
    let end = text.find("<!-- modpack:end -->").unwrap();
    text[start..end].to_owned()
}

#[test]
fn skills_have_a_claude_code_frontmatter() {
    for name in SKILLS {
        let text = skill_template(name);
        let front = text
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .map(|(front, _)| front)
            .unwrap_or_else(|| panic!("{name}: no frontmatter"));
        assert!(front.contains(&format!("name: {name}\n")), "{name}: name");
        let description = front
            .lines()
            .find_map(|l| l.strip_prefix("description: "))
            .unwrap_or_else(|| panic!("{name}: no description"));
        assert!(description.contains("Use when"), "{name}: when to use it");
    }
}

#[test]
fn ai_texts_have_no_em_dash() {
    let mut texts: Vec<String> = SKILLS.iter().map(|n| skill_template(n)).collect();
    texts.push(template(BLOCK));
    for text in texts {
        assert!(!text.contains('\u{2014}'));
    }
}

/// Every `modpack ...` command written in code, in the skills and the block,
/// names a subcommand and flags that exist.
#[test]
fn skills_only_use_existing_commands_and_flags() {
    let mut texts: Vec<(String, String)> = SKILLS
        .iter()
        .map(|n| (n.to_string(), skill_template(n)))
        .collect();
    texts.push((BLOCK.into(), template(BLOCK)));

    let mut checked = 0;
    for (origin, text) in &texts {
        for code in code_of(text) {
            for invocation in invocations(&code) {
                check(origin, &invocation);
                checked += 1;
            }
        }
    }
    // Guards against a parser that silently finds nothing.
    assert!(checked > 20, "only {checked} invocations found");
}

/// Agents run each command of a skill in a fresh shell: a variable set by one
/// command is gone in the next.
#[test]
fn skills_keep_no_state_in_shell_variables() {
    for name in SKILLS {
        for code in code_of(&skill_template(name)) {
            // The first word of each command of the line, after `export`.
            let assigns = code
                .split(['&', ';', '|', '('])
                .filter_map(|command| {
                    let mut words = command.split_whitespace();
                    let first = words.next()?;
                    if first == "export" {
                        words.next()
                    } else {
                        Some(first)
                    }
                })
                .any(|word| {
                    word.split_once('=').is_some_and(|(var, _)| {
                        !var.is_empty()
                            && var.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    })
                });
            assert!(!assigns, "{name}: shell variable set in `{code}`");
        }
    }
}

/// The block names every command of the CLI, down to the nested ones.
#[test]
fn the_block_lists_every_command() {
    fn leaves(cmd: &clap::Command, prefix: &str, out: &mut Vec<String>) {
        for sub in cmd.get_subcommands().filter(|s| s.get_name() != "help") {
            let name = format!("{prefix}{} ", sub.get_name());
            if sub.has_subcommands() {
                leaves(sub, &name, out);
            } else {
                out.push(name.trim_end().to_owned());
            }
        }
    }
    let mut commands = Vec::new();
    leaves(&Cli::command(), "", &mut commands);
    let listed: Vec<String> = code_of(&template(BLOCK))
        .iter()
        .flat_map(|code| invocations(code))
        .map(|words| words.join(" "))
        .collect();
    for command in commands {
        assert!(
            listed
                .iter()
                .any(|l| l == &command || l.starts_with(&format!("{command} "))),
            "`modpack {command}` missing from {BLOCK}"
        );
    }
}

#[test]
fn the_return_skill_reads_the_status_json() {
    let text = skill_template("modpack-return");
    for field in [
        "modpack status --json",
        "instance.path",
        "instance.mode",
        "unknown_jars",
        "drift",
    ] {
        assert!(text.contains(field), "`{field}` missing");
    }
}

#[test]
fn the_audit_skill_records_the_audited_commit() {
    let text = skill_template("modpack-audit");
    for needle in [
        "audit/last.toml",
        "commit = ",
        "modpack pack --out",
        "--json",
    ] {
        assert!(text.contains(needle), "`{needle}` missing");
    }
}

#[test]
fn init_lays_down_the_skills_and_the_block() {
    let (_tmp, root) = init_repo();
    let lock = SocleLock::load(&root).unwrap();
    for name in SKILLS {
        let path = skill_path(name);
        assert_eq!(
            fs::read_to_string(root.join(&path)).unwrap(),
            skill_template(name)
        );
        assert!(
            lock.files.iter().any(|f| f.path == path),
            "{path} not locked"
        );
    }
    let block = agents_block(&root);
    assert_eq!(block.trim(), template(BLOCK).trim());
    for name in SKILLS {
        assert!(block.contains(&format!("`{name}`")));
    }
}

#[test]
fn update_brings_back_the_skills_and_the_block() {
    let (_tmp, root) = init_repo();
    // A repository from a socle that had neither skill and an older block.
    for name in SKILLS {
        fs::remove_dir_all(root.join(format!(".claude/skills/{name}"))).unwrap();
    }
    let mut lock = SocleLock::load(&root).unwrap();
    lock.files
        .retain(|f| !SKILLS.iter().any(|n| f.path == skill_path(n)) && f.path != "AGENTS.md");
    lock.save(&root).unwrap();
    let agents = root.join("AGENTS.md");
    let old = fs::read_to_string(&agents)
        .unwrap()
        .replace(&agents_block(&root), "Old instructions.\n");
    fs::write(&agents, old).unwrap();
    let mut config = SocleConfig::load(&root).unwrap();
    config.version = "0.0.1".into();
    config.save(&root).unwrap();

    modpack(&root).arg("update").assert().success();

    for name in SKILLS {
        assert_eq!(
            fs::read_to_string(root.join(skill_path(name))).unwrap(),
            skill_template(name)
        );
    }
    assert_eq!(agents_block(&root).trim(), template(BLOCK).trim());
}

/// The code of a Markdown text: fenced blocks line by line, then inline spans.
fn code_of(text: &str) -> Vec<String> {
    let mut code = Vec::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            code.push(line.to_owned());
        } else {
            code.extend(line.split('`').skip(1).step_by(2).map(str::to_owned));
        }
    }
    code
}

/// The words after each `modpack ` of a code line, up to a shell separator.
fn invocations(code: &str) -> Vec<Vec<String>> {
    let mut found = Vec::new();
    let mut rest = code;
    while let Some(at) = rest.find("modpack ") {
        let preceded = rest[..at]
            .chars()
            .last()
            .is_some_and(|c| c.is_alphanumeric() || c == '-' || c == '/' || c == '.');
        rest = &rest[at + "modpack ".len()..];
        if preceded {
            continue;
        }
        let end = rest.find(['|', ';', ')', '&', '#']).unwrap_or(rest.len());
        found.push(
            rest[..end]
                .split_whitespace()
                .map(|w| w.trim_matches(|c| c == '[' || c == ']').to_owned())
                .collect(),
        );
    }
    found
}

fn check(origin: &str, words: &[String]) {
    let root = Cli::command();
    let mut cmd = &root;
    let mut words = words.iter().peekable();
    let first = words
        .next()
        .unwrap_or_else(|| panic!("{origin}: bare `modpack`"));
    cmd = cmd
        .find_subcommand(first)
        .unwrap_or_else(|| panic!("{origin}: unknown command `modpack {first}`"));
    while let Some(next) = words.peek() {
        match cmd.find_subcommand(next.as_str()) {
            Some(sub) => {
                cmd = sub;
                words.next();
            }
            None => break,
        }
    }
    assert!(
        !cmd.has_subcommands(),
        "{origin}: `modpack {first}` needs a subcommand"
    );
    for word in words {
        let Some(flag) = word.strip_prefix("--") else {
            continue;
        };
        let flag = flag.split('=').next().unwrap();
        let known = cmd
            .get_arguments()
            .chain(root.get_arguments())
            .any(|a| a.get_long() == Some(flag));
        assert!(
            known,
            "{origin}: `modpack {}` has no --{flag}",
            cmd.get_name()
        );
    }
}
