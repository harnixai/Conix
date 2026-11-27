//! 随 Conix 带的流程包（docs/design/flows.md §7.12）：源头是仓库里的演示项目 `examples/xforge/.conix/flows/`，
//! 构建时编进来（`build.rs`）。它们不直接参与哪个项目的运行，只是全局库的种子：`conix` 起来时解进
//! `~/.conix/flows/`（[`super::library::Library::bootstrap`]），项目要用就 `conix flow install`。
//! 每个内置包当作一个包文件看：内容与导出同一套规矩（[`package::excluded`]），摘要与导出它的 MANIFEST 的一样。

use std::path::Path;

use super::package;

include!(concat!(env!("OUT_DIR"), "/builtin_bundles.rs"));

/// 一个内置包：名字、`bundle.yaml` 的版本、内容与摘要。
#[derive(Debug, Clone)]
pub struct Builtin {
    pub name: String,
    pub version: String,
    pub files: package::Files,
    pub digest: String,
}

/// 内置的流程包的名字。
pub fn bundles() -> Vec<&'static str> {
    let mut names: Vec<&str> = FILES
        .iter()
        .filter_map(|(rel, _)| rel.split('/').next())
        .collect();
    names.dedup();
    names
}

/// 一个内置包；没有这个名字的就是 `None`。
pub fn package(name: &str) -> Option<Builtin> {
    let prefix = format!("{name}/");
    let mut files: package::Files = FILES
        .iter()
        .filter_map(|(rel, text)| {
            let rel = rel.strip_prefix(&prefix)?;
            let parts: Vec<&str> = rel.split('/').collect();
            (!package::excluded(&parts)).then(|| (rel.to_string(), text.as_bytes().to_vec()))
        })
        .collect();
    if files.is_empty() {
        return None;
    }
    files.sort();
    let version = files
        .iter()
        .find(|(p, _)| p == harnix_flow::bundle::BUNDLE_FILE)
        .and_then(|(_, b)| serde_yaml::from_slice::<serde_yaml::Value>(b).ok())
        .and_then(|v| v["version"].as_str().map(String::from))
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "0.0.0".into());
    let digest = package::digest_of(&files);
    Some(Builtin {
        name: name.to_string(),
        version,
        files,
        digest,
    })
}

/// 全部内置包。
pub fn packages() -> Vec<Builtin> {
    bundles().into_iter().filter_map(package).collect()
}

/// 把内置的流程包原样写到 `dir`（先清空）：测试与演示用。
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
    #[test]
    fn xforge_is_built_in_as_a_package_that_validates() {
        assert_eq!(super::bundles(), ["xforge"]);
        let p = super::package("xforge").unwrap();
        assert!(!p.version.is_empty() && p.version != "0.0.0");
        assert!(p.files.iter().any(|(f, _)| f == "commands/xspec.js"));
        assert!(super::package("nope").is_none());
        let staged = super::package::stage(
            "xforge",
            &p.files,
            None,
            None,
            super::package::default_catalog(),
        )
        .unwrap();
        assert_eq!(staged.digest, p.digest);
        assert_eq!(staged.version(), p.version);
    }
}
