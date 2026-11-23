//! LIVE：命令行子命令对着真实实例（需求 I-03、I-04、L-09、L-11）——`codix list` 的纯文本、`codix stop`、
//! `list --all`；首次运行写出带注释的缺省配置，`list` 的行按语言显示。不调模型。

mod live;

use live::Live;

/// 跑一次 `codix <args>`，返回 stdout。`CODIX_LANG` 指定语言，其余走用例自己的 `CODIX_HOME`。
fn cli(home: &std::path::Path, lang: &str, args: &[&str]) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_codix"))
        .args(args)
        .env("CODIX_HOME", home)
        .env("CODIX_LANG", lang)
        .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "codix {args:?}：{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn list_and_stop_work_from_the_command_line() {
    let lv = Live::new();
    let c = lv.start().await;
    let pid = lv.read("instance.json")["pid"].as_u64().unwrap();
    let home = lv.home.path();

    // 接入着：一行里有名字、pid、项目、谁接入的。
    let text = cli(home, "en", &["list"]);
    let row = text
        .lines()
        .find(|l| l.starts_with(live::NAME))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(row.contains(&format!("pid {pid}")), "{row}");
    assert!(row.contains(&lv.project_path()), "{row}");
    assert!(row.contains("attached:"), "{row}");

    // 语言：同一行换成中文，词序与措辞来自语言资源。
    let zh = cli(home, "zh", &["list"]);
    assert!(
        !zh.contains("attached:") && zh.contains(&pid.to_string()),
        "{zh}"
    );

    // 脱离之后没有接入者那一段。
    drop(c);
    let mut shown = String::new();
    for _ in 0..50 {
        shown = cli(home, "en", &["list"]);
        if !shown.contains("attached:") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(!shown.contains("attached:"), "{shown}");

    // `codix stop`：实例进程没了，`list` 不再列，`list --all` 还列着（状态是停止）。
    let said = cli(home, "en", &["stop", live::NAME]);
    assert!(
        said.contains("Stopped") && said.contains(live::NAME),
        "{said}"
    );
    assert!(
        !cli(home, "en", &["list"])
            .lines()
            .any(|l| l.starts_with(live::NAME)),
        "停掉的不在运行的列表里"
    );
    let all = cli(home, "en", &["list", "--all"]);
    let row = all
        .lines()
        .find(|l| l.starts_with(live::NAME))
        .unwrap_or_else(|| panic!("{all}"));
    assert!(row.contains("Stopped"), "{row}");
    assert!(!row.contains(&format!("pid {pid} ")), "{row}");
}

#[test]
#[ignore = "live"]
fn the_first_run_writes_a_commented_default_config_in_the_chosen_language() {
    for lang in ["en", "zh"] {
        let home = tempfile::tempdir().unwrap();
        cli(home.path(), lang, &["list"]);
        let yaml = std::fs::read_to_string(home.path().join("codix.yaml"))
            .unwrap_or_else(|e| panic!("{lang}：{e}"));
        assert!(yaml.contains('#'), "带注释：{yaml}");
        assert!(yaml.contains("ui:") && yaml.contains("instance:"), "{yaml}");
        let cjk = yaml.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c));
        assert_eq!(cjk, lang == "zh", "{lang} 的模板：{yaml}");
    }
}
