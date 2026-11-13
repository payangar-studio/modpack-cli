//! Writing `variants.toml`. The file is written by hand: edits go through
//! `toml_edit`, which keeps its comments and layout.

use std::io::Write;
use std::path::Path;

use toml_edit::{Array, DocumentMut, Item, TableLike, Value};

use crate::error::{Context, Error, Result};
use crate::model::variants::{self, DEFAULT_VARIANT};

const HINT: &str = "fix variants.toml";

/// Removes `key` from the `remove` of the default variant and from the `add` and
/// `remove` of every variant. Returns whether the text changed.
pub fn remove_key(doc: &mut DocumentMut, key: &str) -> bool {
    let mut changed = false;
    if let Some(default) = doc.get_mut("default").and_then(Item::as_table_like_mut) {
        changed |= drop_value(default, "remove", key);
    }
    if let Some(all) = doc.get_mut("variants").and_then(Item::as_table_like_mut) {
        for (_, variant) in all.iter_mut() {
            if let Some(variant) = variant.as_table_like_mut() {
                changed |= drop_value(variant, "add", key);
                changed |= drop_value(variant, "remove", key);
            }
        }
    }
    changed
}

/// Makes `key` a mod of `variant` only: removed by the default variant, added by
/// `variant`. `variant` must already exist.
pub fn move_to_variant(doc: &mut DocumentMut, key: &str, variant: &str) -> Result<()> {
    let target = doc
        .get_mut("variants")
        .and_then(Item::as_table_like_mut)
        .and_then(|all| all.get_mut(variant))
        .and_then(Item::as_table_like_mut)
        .ok_or_else(|| unknown_variant(variant))?;
    push_value(target, "add", key)?;
    let default = doc
        .entry("default")
        .or_insert_with(toml_edit::table)
        .as_table_like_mut()
        .ok_or_else(|| Error::usage("variants.toml: `default` is not a table", HINT))?;
    push_value(default, "remove", key)
}

/// Loads `<root>/variants.toml`, applies `edit`, and writes the file back if the
/// text changed. A missing file is an empty one, written only if `edit`
/// changes it.
pub fn edit<T>(root: &Path, edit: impl FnOnce(&mut DocumentMut) -> Result<T>) -> Result<T> {
    let path = root.join(variants::FILE_NAME);
    let before = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e).context(format!("reading {}", path.display())),
    };
    let mut doc: DocumentMut = before.parse().map_err(|e: toml_edit::TomlError| {
        Error::usage(format!("variants.toml is invalid: {}", e.message()), HINT).with_source(e)
    })?;
    let result = edit(&mut doc)?;
    let after = doc.to_string();
    if after != before {
        write_atomic(&path, &after)?;
    }
    Ok(result)
}

/// The names of the variants `variants.toml` defines, the default one first.
pub fn variant_names(root: &Path) -> Result<Vec<String>> {
    let path = root.join(variants::FILE_NAME);
    let variants = if path.is_file() {
        variants::Variants::load(&path)?
    } else {
        variants::Variants::parse("")?
    };
    Ok(variants.names().into_iter().map(str::to_owned).collect())
}

/// Fails unless `variant` is a variant of `variants.toml` other than the default one.
pub fn require_variant(root: &Path, variant: &str) -> Result<()> {
    let names = variant_names(root)?;
    if variant != DEFAULT_VARIANT && names.iter().any(|n| n == variant) {
        return Ok(());
    }
    Err(Error::usage(
        format!("unknown variant `{variant}`"),
        format!(
            "use one of: {} (declare a new variant in variants.toml first)",
            names
                .iter()
                .filter(|n| *n != DEFAULT_VARIANT)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ),
    ))
}

fn unknown_variant(variant: &str) -> Error {
    Error::usage(
        format!("variants.toml has no variant `{variant}`"),
        "declare it as [variants.<name>] in variants.toml first",
    )
}

fn drop_value(table: &mut dyn TableLike, field: &str, key: &str) -> bool {
    let Some(array) = table.get_mut(field).and_then(Item::as_array_mut) else {
        return false;
    };
    let before = array.len();
    array.retain(|v| v.as_str() != Some(key));
    if array.len() == before {
        return false;
    }
    // The element that became first keeps the space it had after a comma.
    if let Some(first) = array.get_mut(0) {
        first.decor_mut().set_prefix("");
    }
    if array.is_empty() {
        array.set_trailing("");
        array.set_trailing_comma(false);
    }
    true
}

fn push_value(table: &mut dyn TableLike, field: &str, key: &str) -> Result<()> {
    if table.get(field).is_none() {
        table.insert(field, Item::Value(Value::Array(Array::new())));
    }
    let array = table
        .get_mut(field)
        .and_then(Item::as_array_mut)
        .ok_or_else(|| Error::usage(format!("variants.toml: `{field}` is not a list"), HINT))?;
    if !array.iter().any(|v| v.as_str() == Some(key)) {
        array.push(key);
    }
    Ok(())
}

fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .context(format!("creating a temporary file in {}", dir.display()))?;
    tmp.write_all(text.as_bytes())
        .context(format!("writing {}", path.display()))?;
    tmp.persist(path)
        .map_err(|e| e.error)
        .context(format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repo/variants.toml");
        std::fs::read_to_string(path).unwrap()
    }

    fn doc(text: &str) -> DocumentMut {
        text.parse().unwrap()
    }

    #[test]
    fn removing_a_key_keeps_comments_and_layout() {
        let before = fixture();
        let mut doc = doc(&before);
        assert!(remove_key(&mut doc, "sodium"));
        let after = doc.to_string();
        assert_eq!(
            after,
            before.replace(
                "remove = [\"sodium\", \"iris\", \"jei\"]",
                "remove = [\"iris\", \"jei\"]"
            )
        );
        assert!(after.contains("# Global conditions: every variant, the default one included."));
    }

    #[test]
    fn removing_a_key_cleans_every_list() {
        let mut doc = doc(&fixture());
        assert!(remove_key(&mut doc, "hardcore-tweaks"));
        let after = doc.to_string();
        assert!(!after.contains("hardcore-tweaks"), "{after}");
        assert!(after.contains("[default]\ndescription = \"Full pack\"\nremove = []\n"));
        assert!(after.contains("description = \"Hardcore\"\nadd = []\n"));
        variants::Variants::parse(&after).unwrap();
    }

    #[test]
    fn removing_an_unlisted_key_changes_nothing() {
        let before = fixture();
        let mut doc = doc(&before);
        assert!(!remove_key(&mut doc, "nope"));
        assert_eq!(doc.to_string(), before);
    }

    #[test]
    fn last_element_and_inline_variants() {
        let mut doc = doc("variants = { a = { remove = [\"x\", \"y\"] } }\n");
        assert!(remove_key(&mut doc, "y"));
        assert_eq!(doc.to_string(), "variants = { a = { remove = [\"x\"] } }\n");
    }

    #[test]
    fn moving_to_a_variant_updates_both_lists() {
        let before = fixture();
        let mut doc = doc(&before);
        move_to_variant(&mut doc, "sodium", "hardcore").unwrap();
        let after = doc.to_string();
        assert!(
            after.contains("remove = [\"hardcore-tweaks\", \"sodium\"]"),
            "{after}"
        );
        assert!(
            after.contains("add = [\"hardcore-tweaks\", \"sodium\"]"),
            "{after}"
        );
        assert!(after.starts_with("# Variants of the fixture pack."));
        // Idempotent.
        move_to_variant(&mut doc, "sodium", "hardcore").unwrap();
        assert_eq!(doc.to_string(), after);
    }

    #[test]
    fn moving_creates_missing_lists() {
        let mut doc =
            doc("# pack variants\n[default]\ndescription = \"Full\"\n\n[variants.server]\n");
        move_to_variant(&mut doc, "jei", "server").unwrap();
        assert_eq!(
            doc.to_string(),
            "# pack variants\n[default]\ndescription = \"Full\"\nremove = [\"jei\"]\n\n[variants.server]\nadd = [\"jei\"]\n"
        );
        let mut doc = self::doc("[variants.server]\n");
        move_to_variant(&mut doc, "jei", "server").unwrap();
        let parsed = variants::Variants::parse(&doc.to_string()).unwrap();
        assert_eq!(parsed.default.remove, ["jei"]);
    }

    #[test]
    fn moving_to_an_unknown_variant_fails() {
        let mut doc = doc(&fixture());
        let err = move_to_variant(&mut doc, "jei", "lowend").unwrap_err();
        assert!(err.to_string().contains("no variant `lowend`"));
    }

    #[test]
    fn edit_writes_only_on_change() {
        let root = tempfile::tempdir().unwrap();
        edit(root.path(), |doc| Ok(remove_key(doc, "x"))).unwrap();
        assert!(!root.path().join(variants::FILE_NAME).exists());
        std::fs::write(root.path().join(variants::FILE_NAME), fixture()).unwrap();
        assert!(edit(root.path(), |doc| Ok(remove_key(doc, "jei"))).unwrap());
        let text = std::fs::read_to_string(root.path().join(variants::FILE_NAME)).unwrap();
        assert!(text.contains("remove = [\"sodium\", \"iris\"]"));
    }

    #[test]
    fn require_variant_lists_the_known_variants() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(variants::FILE_NAME), fixture()).unwrap();
        require_variant(root.path(), "server").unwrap();
        let err = require_variant(root.path(), "lowend").unwrap_err();
        assert!(err.hint().contains("hardcore, server"));
        assert!(require_variant(root.path(), DEFAULT_VARIANT).is_err());
    }
}
