//! 随 Conix 内置的流程包（docs/design/layout.md §2）：源头是仓库里的演示项目 `examples/xforge/.conix/flows/`，
//! 构建时编进来（`build.rs`）。实例起来时整个重写到项目的 `.conix/state/conix/flows-builtin/`，流程库当作内置的
//! 一层读（`DirPacks`，来源 `builtin`）。整个重写：一个项目同时只有一个实例，换了版本删掉的文件不会留下。

use std::path::Path;

include!(concat!(env!("OUT_DIR"), "/builtin_bundles.rs"));

/// 内置的流程包的名字。
pub fn bundles() -> Vec<&'static str> {
    let mut names: Vec<&str> = FILES
        .iter()
        .filter_map(|(rel, _)| rel.split('/').next())
        .collect();
    names.dedup();
    names
}

/// 把内置的流程包写到 `dir`（先清空）。
pub fn write(dir: &Path) -> std::io::Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    for (rel, text) in FILES {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, text)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use harnix_flow::Packs;

    #[test]
    fn xforge_is_built_in_and_offered_as_a_bundle() {
        assert_eq!(super::bundles(), ["xforge"]);
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("stale.txt"), "left from an older version").unwrap();
        super::write(d.path()).unwrap();
        assert!(!d.path().join("stale.txt").exists(), "rewritten whole");
        let packs = harnix_flow::DirPacks::new([d.path()]);
        let flows = packs.flows();
        assert!(
            flows.contains(&"xforge/solid".to_string())
                && flows.contains(&"xforge/quick".to_string()),
            "{flows:?}"
        );
        assert_eq!(packs.origin("xforge/solid"), harnix_flow::Origin::Builtin);
        assert!(packs.bundle_file("xforge", "commands/xspec.js").is_some());
    }
}
