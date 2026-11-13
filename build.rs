// Rebuild when a template is added or removed: `include_dir!` only tracks the
// files that existed at the previous build.
fn main() {
    println!("cargo:rerun-if-changed=templates");
}
