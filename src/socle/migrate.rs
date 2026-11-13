//! Migrations `modpack update` runs before writing the socle: a layout an
//! older CLI wrote, renamed to the current one.
//!
//! Modes became variants: `modes.toml` is `variants.toml`, its `[modes.<name>]`
//! tables are `[variants.<name>]`, and the instance state records `variant`
//! instead of `mode`. `overrides/<name>/` is named after the variant, so it
//! does not move.

use std::fs;
use std::path::Path;

use toml_edit::{DocumentMut, Key};

use crate::error::{Context, Error, Result};
use crate::model::instance::{InstanceState, Instances, STATE_FILE};
use crate::model::variants;

pub(crate) const OLD_FILE: &str = "modes.toml";
const OLD_DOC: &str = "docs/modes.md";
const NEW_DOC: &str = "docs/variants.md";
/// The comments `modpack init` wrote in `modes.toml`, line by line, and what
/// they read in the `variants.toml` it writes now.
const TEMPLATE_COMMENTS: &[(&str, &str)] = &[
    (
        "# Modes of the pack. The default mode holds every mod of mods.lock.toml but",
        "# Variants of the pack. The default variant holds every mod of mods.lock.toml but",
    ),
    (
        "# those it removes; every other mode starts from the default selection.",
        "# those it removes; every other variant starts from the default selection.",
    ),
    (
        "# global: every mode, the default one included",
        "# global: every variant, the default one included",
    ),
    ("# [modes.", "# [variants."),
    (OLD_DOC, NEW_DOC),
];

/// Runs every migration the repository of the pack `name` and this machine
/// need, and returns one line per change made, for the user.
pub fn run(root: &Path, name: &str, instances_file: &Path) -> Result<Vec<String>> {
    let mut done = Vec::new();
    if modes_file(root)? {
        done.push(format!(
            "Renamed {OLD_FILE} to {}: its [modes.<name>] tables are now [variants.<name>]. \
             If the repository sets the Actions variable MODPACK_PUBLISH_MODES, rename it \
             MODPACK_PUBLISH_VARIANTS: the publish workflow no longer reads it.",
            variants::FILE_NAME
        ));
    }
    if let Some(dir) = Instances::existing(instances_file, name)?
        && state_file(&dir)?
    {
        done.push(format!(
            "Renamed `mode` to `variant` in {}.",
            dir.join(STATE_FILE).display()
        ));
    }
    Ok(done)
}

/// `modes.toml` to `variants.toml`, comments and layout kept. Returns whether
/// the old file was there.
fn modes_file(root: &Path) -> Result<bool> {
    let old = root.join(OLD_FILE);
    if !old.is_file() {
        return Ok(false);
    }
    let new = root.join(variants::FILE_NAME);
    if new.exists() {
        return Err(Error::usage(
            format!(
                "both {OLD_FILE} and {} are in the repository",
                variants::FILE_NAME
            ),
            format!(
                "move what {OLD_FILE} holds into {}, delete {OLD_FILE}, then run `modpack update` again",
                variants::FILE_NAME
            ),
        ));
    }
    let text = fs::read_to_string(&old).context(format!("reading {}", old.display()))?;
    let migrated = rename_modes_table(&text)?;
    fs::write(&new, migrated).context(format!("writing {}", new.display()))?;
    fs::remove_file(&old).context(format!("removing {}", old.display()))?;
    Ok(true)
}

/// The text of `modes.toml` with its `modes` table renamed `variants`, and
/// the comments `modpack init` wrote, the link to the old reference included,
/// in the words of variants.
fn rename_modes_table(text: &str) -> Result<String> {
    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| {
        Error::usage(
            format!("{OLD_FILE} is invalid: {}", e.message()),
            format!("fix {OLD_FILE}, then run `modpack update` again"),
        )
        .with_source(e)
    })?;
    if let Some((key, item)) = doc.remove_entry("modes") {
        let mut renamed = Key::new("variants");
        *renamed.leaf_decor_mut() = key.leaf_decor().clone();
        *renamed.dotted_decor_mut() = key.dotted_decor().clone();
        doc.insert_formatted(&renamed, item);
    }
    Ok(TEMPLATE_COMMENTS
        .iter()
        .fold(doc.to_string(), |text, (old, new)| text.replace(old, new)))
}

/// The instance state's `mode` to `variant`. Returns whether it had one.
fn state_file(instance_dir: &Path) -> Result<bool> {
    let path = instance_dir.join(STATE_FILE);
    if !path.is_file() {
        return Ok(false);
    }
    let text = fs::read_to_string(&path).context(format!("reading {}", path.display()))?;
    let hint = format!(
        "delete {}, then load a variant with `modpack variant load <variant> --force`",
        path.display()
    );
    let mut table: toml::Table = text.parse().map_err(|e: toml::de::Error| {
        Error::usage(
            format!("{} is invalid: {}", path.display(), e.message()),
            &hint,
        )
        .with_source(e)
    })?;
    let Some(mode) = table.remove("mode") else {
        return Ok(false);
    };
    table.entry("variant").or_insert(mode);
    let state: InstanceState =
        toml::Value::Table(table)
            .try_into()
            .map_err(|e: toml::de::Error| {
                Error::usage(
                    format!("{} is invalid: {}", path.display(), e.message()),
                    &hint,
                )
                .with_source(e)
            })?;
    state.save(instance_dir)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::socle::{InitValues, SocleConfig};

    const MODES: &str = "\
# Modes of the pack.
# https://github.com/payangar-studio/modpack-cli/blob/main/docs/modes.md

[conditions]
deps-complete = true

[default]
description = \"Full pack\"

# The server.
[modes.server]
remove = [\"sodium\"] # client only

[modes.server.conditions]
side = \"server\"

[modes.hardcore]
add = []
";

    #[test]
    fn modes_tables_become_variants_tables_with_comments_kept() {
        let text = rename_modes_table(MODES).unwrap();
        assert_eq!(
            text,
            MODES
                .replace("[modes.", "[variants.")
                .replace("docs/modes.md", "docs/variants.md")
        );
        let parsed = variants::Variants::parse(&text).unwrap();
        assert_eq!(parsed.names(), ["default", "hardcore", "server"]);
    }

    #[test]
    fn the_init_template_becomes_the_variants_template() {
        const OLD_TEMPLATE: &str = "\
# Modes of the pack. The default mode holds every mod of mods.lock.toml but
# those it removes; every other mode starts from the default selection.
# Reference of the syntax and of the conditions:
# https://github.com/payangar-studio/modpack-cli/blob/main/docs/modes.md
#
# [conditions]                  # global: every mode, the default one included
# deps-complete = true
#
# [modes.server]
# description = \"Dedicated server\"
# remove = [\"sodium\"]
#
# [modes.server.conditions]
# side = \"server\"

[default]
description = \"Full pack\"
";
        let new_template = include_str!("../../templates/project/variants.toml");
        assert_eq!(rename_modes_table(OLD_TEMPLATE).unwrap(), new_template);
    }

    #[test]
    fn a_file_without_modes_is_kept_as_is() {
        let text = "[default]\ndescription = \"Full pack\"\n";
        assert_eq!(rename_modes_table(text).unwrap(), text);
    }

    /// A repository named `Test Pack`, and an instance of it registered in
    /// `instances.toml`, both under `tmp`.
    fn repo_and_instance(
        tmp: &Path,
    ) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let root = tmp.join("pack");
        fs::create_dir_all(&root).unwrap();
        SocleConfig {
            version: "0.1.0".into(),
            init: InitValues {
                name: "Test Pack".into(),
                minecraft: "1.21.1".into(),
                loader: "neoforge".into(),
                loader_version: "21.1.77".into(),
                author: "Tester".into(),
            },
        }
        .save(&root)
        .unwrap();
        let instance = tmp.join("instance");
        fs::create_dir_all(&instance).unwrap();
        let instances_file = tmp.join("instances.toml");
        let mut instances = Instances::default();
        instances.set("Test Pack", instance.clone());
        instances.save(&instances_file).unwrap();
        (root, instance, instances_file)
    }

    #[test]
    fn migrates_the_repository_and_the_instance_state_once() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, instance, instances_file) = repo_and_instance(tmp.path());
        fs::write(root.join(OLD_FILE), MODES).unwrap();
        fs::write(
            instance.join(STATE_FILE),
            "mode = \"server\"\n\n[files]\n\"mods/a.jar\" = \"abc\"\n",
        )
        .unwrap();

        let done = run(&root, "Test Pack", &instances_file).unwrap();
        assert_eq!(done.len(), 2, "{done:?}");
        assert!(!root.join(OLD_FILE).exists());
        assert!(
            fs::read_to_string(root.join(variants::FILE_NAME))
                .unwrap()
                .contains("[variants.server]")
        );
        let state = InstanceState::load(&instance).unwrap();
        assert_eq!(state.variant.as_deref(), Some("server"));
        assert_eq!(state.files["mods/a.jar"], "abc");

        assert_eq!(
            run(&root, "Test Pack", &instances_file).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn both_files_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, _, instances_file) = repo_and_instance(tmp.path());
        fs::write(root.join(OLD_FILE), MODES).unwrap();
        fs::write(root.join(variants::FILE_NAME), "[default]\n").unwrap();
        let err = run(&root, "Test Pack", &instances_file).unwrap_err();
        assert!(
            err.to_string()
                .contains("both modes.toml and variants.toml")
        );
        assert!(root.join(OLD_FILE).exists());
    }
}
