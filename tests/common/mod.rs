//! Helpers shared by the integration tests.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

/// Writes an executable script at `path`, through `cp`, so that this process
/// never holds a write descriptor on it: a test spawning a process meanwhile
/// would hand that descriptor to its child until the child's exec, and
/// running the script would fail with "Text file busy" (ETXTBSY).
pub fn write_executable(path: &Path, contents: &str) {
    let source = path.with_extension("src");
    fs::write(&source, contents).unwrap();
    let copied = Command::new("cp").arg(&source).arg(path).status().unwrap();
    assert!(copied.success(), "cp {} failed", path.display());
    fs::remove_file(&source).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
