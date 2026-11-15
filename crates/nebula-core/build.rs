//! Embeds `ui/dist/vendor` (pdf.js, CMaps, standard fonts) into the binary so
//! the node stays a zero-install single executable on every platform.

use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let vendor = manifest.join("../../ui/dist/vendor");
    println!("cargo:rerun-if-changed={}", vendor.display());

    let mut entries = Vec::new();
    if vendor.is_dir() {
        collect(&vendor, &vendor, &mut entries);
    }
    entries.sort();

    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("vendor_assets.rs");
    let mut code = String::from("/// (url path relative to /vendor, embedded bytes)\n");
    code.push_str("pub static VENDOR_FILES: &[(&str, &[u8])] = &[\n");
    for (rel, abs) in &entries {
        let abs = abs
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        code.push_str(&format!("    ({rel:?}, include_bytes!({abs:?})),\n"));
    }
    code.push_str("];\n");
    std::fs::write(&out, code).expect("write vendor_assets.rs");
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out);
        } else if path.is_file() {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            out.push((rel, path));
        }
    }
}
