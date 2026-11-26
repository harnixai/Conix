//! 命令行帮助按语言显示（需求 L-11）：起真实的二进制、切到英文。

fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c, '\u{4e00}'..='\u{9fff}' | '\u{3000}'..='\u{303f}' | '\u{ff00}'..='\u{ffef}')
    })
}

fn help_of(args: &[&str]) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_conix"))
        .args(args)
        .env("CONIX_LANG", "en")
        .env("CONIX_HOME", tempfile::tempdir().unwrap().path())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn cli_help_is_english() {
    let text = help_of(&["--help"]);
    assert!(text.contains("agent workbench"), "{text}");
    assert!(!has_cjk(&text), "{text}");
    for sub in ["new", "list", "attach", "start", "stop"] {
        let text = help_of(&[sub, "--help"]);
        assert!(!has_cjk(&text), "{sub}: {text}");
    }
    assert!(help_of(&["attach", "--help"]).contains("Kick out"));
}

fn conix(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_conix"))
        .args(args)
        .env("CONIX_LANG", "en")
        .env("CONIX_HOME", home)
        .output()
        .unwrap()
}

#[test]
fn flow_export_inspect_and_import_work_without_an_instance() {
    for sub in ["flow", "flow export", "flow import", "flow inspect"] {
        let mut a: Vec<&str> = sub.split(' ').collect();
        a.push("--help");
        assert!(!has_cjk(&help_of(&a)), "{sub}");
    }
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let zip = work.path().join("xforge.zip");
    let (zip_s, proj_s) = (zip.to_str().unwrap(), project.path().to_str().unwrap());
    let o = conix(
        home.path(),
        &["flow", "export", "xforge", "-o", zip_s, "--project", proj_s],
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let o = conix(home.path(), &["flow", "inspect", zip_s]);
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        o.status.success() && out.contains("format: 1") && out.contains("Verified"),
        "{out}"
    );
    let o = conix(home.path(), &["flow", "import", zip_s, "--project", proj_s]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(
        project
            .path()
            .join(".conix/flows/xforge/bundle.yaml")
            .is_file()
    );
    let o = conix(home.path(), &["flow", "import", zip_s, "--project", proj_s]);
    assert!(!o.status.success() && String::from_utf8_lossy(&o.stderr).contains("--force"));
    let o = conix(
        home.path(),
        &["flow", "import", zip_s, "--force", "--project", proj_s],
    );
    assert!(o.status.success());
    assert!(
        project
            .path()
            .join(".conix/state/conix/flow-backups")
            .is_dir()
    );
}
