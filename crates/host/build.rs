//! 随 Conix 内置的流程包：`examples/xforge/.conix/flows/` 下带 `bundle.yaml` 的每个目录（docs/design/layout.md）。
//! 演示项目就是它们的源头，改那里就是改内置的；这里把每个文件编进二进制（`builtin_bundles.rs`），`conix` 起来时
//! 解进全局库 `~/.conix/flows/`（`flows::builtin`、`flows::library`）。

use std::path::{Path, PathBuf};

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/xforge/.conix/flows");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = vec![];
    if let Ok(entries) = std::fs::read_dir(&root) {
        let mut bundles: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("bundle.yaml").is_file())
            .collect();
        bundles.sort();
        for b in bundles {
            walk(&b, &root, &mut files);
        }
    }
    files.sort();
    let mut out = String::from(
        "/// 内置流程包的文件：相对流程包目录的路径与它的内容。\npub static FILES: &[(&str, &str)] = &[\n",
    );
    for (rel, abs) in &files {
        println!("cargo:rerun-if-changed={}", abs.display());
        out.push_str(&format!(
            "    ({rel:?}, include_str!({:?})),\n",
            abs.display().to_string()
        ));
    }
    out.push_str("];\n");
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("builtin_bundles.rs");
    std::fs::write(dest, out).unwrap();
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, PathBuf)>) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        if p.is_dir() {
            walk(&p, root, out);
        } else if p.is_file() {
            let rel = p
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, p.canonicalize().unwrap()));
        }
    }
}
