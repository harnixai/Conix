//! 自带插件 `xforge`（plugins/xforge，docs/design/plugins.md §6）在真实嵌入的 Agix 上：装 zip、启用后
//! quick / solid 两个流程都挂得上；再按 Conix 调函数的样子（`plugin.call`，门禁只读、步骤可写）把一次变更
//! 从开到归档走完。不调模型：模型的活（写三份 .md）由测试直接写文件代替。
//! 单独一个测试文件：一个进程只能嵌入一份引擎。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use host::hub::Registry;
use host::{Hub, HubConfig, Service};
use port::{Command, FlowOrigin, Reply};
use serde_json::{Value, json};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// 跑 scripts/build-plugins.sh，返回 xforge 的 zip。
fn build_zip() -> PathBuf {
    let out = std::process::Command::new(root().join("scripts/build-plugins.sh"))
        .output()
        .expect("build-plugins.sh");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    stdout
        .lines()
        .map(PathBuf::from)
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("xforge-"))
        })
        .expect("xforge zip")
}

fn git(dir: &Path, args: &[&str]) {
    let st = std::process::Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git");
    assert!(st.success(), "git {args:?}");
}

struct Ctx {
    client: wire::Client,
    project: PathBuf,
}

impl Ctx {
    async fn call(&self, function: &str, input: Value, writes: bool, rules: Value) -> Value {
        self.client
            .call(
                "plugin.call",
                json!({"function": function, "input": input, "cwd": self.project,
                       "writes": writes, "permission_rules": rules}),
            )
            .await
            .unwrap()
    }

    /// 门禁：只读调用，没有额外规则。
    async fn gate(&self, gate: &str, station: &str, change: &str) -> Value {
        self.gate_with(gate, station, change, false, json!([]))
            .await
    }

    async fn gate_with(
        &self,
        gate: &str,
        station: &str,
        change: &str,
        writes: bool,
        rules: Value,
    ) -> Value {
        let input = json!({"gate": gate, "run": "r1", "flow": "xforge-solid", "station": station,
            "inputs": {}, "outputs": {"open": {"change": change}},
            "workspace": self.project, "project_root": self.project});
        let r = self.call("xforge.check", input, writes, rules).await;
        assert_eq!(r["ok"], true, "{r}");
        r["value"].clone()
    }

    /// `xforge.command`：回命令的信封（`{ok, data}` 或 `{ok:false, error}`）。
    async fn cmd(&self, argv: &[&str], change: Option<&str>, writes: bool) -> Value {
        let r = self
            .call(
                "xforge.command",
                json!({"argv": argv, "change": change}),
                writes,
                json!([]),
            )
            .await;
        assert_eq!(r["ok"], true, "{r}");
        r["value"].clone()
    }

    async fn ok(&self, argv: &[&str], change: &str) -> Value {
        let r = self.cmd(argv, Some(change), true).await;
        assert_eq!(r["ok"], true, "{argv:?}: {r}");
        r["data"].clone()
    }

    fn file(&self, rel: &str) -> String {
        std::fs::read_to_string(self.project.join(rel)).unwrap_or_default()
    }
}

fn step_input(project: &Path, step: &str, change: Option<&str>) -> Value {
    json!({"step": step, "run": "r1", "flow": "xforge-solid", "station": step.trim_start_matches("xforge-"),
        "inputs": {"title": "Add Widget support", "risk": "medium"},
        "outputs": change.map(|c| json!({"open": {"change": c}})).unwrap_or(json!({})),
        "workspace": project, "project_root": project})
}

#[tokio::test(flavor = "multi_thread")]
async fn xforge_installs_offers_both_flows_and_carries_a_change_from_open_to_archive() {
    let home = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    let zip = build_zip();
    std::fs::copy(&zip, store.path().join(zip.file_name().unwrap())).unwrap();

    git(&project_path, &["init", "-q"]);
    std::fs::write(project_path.join("README.md"), "hello\n").unwrap();
    git(&project_path, &["add", "README.md"]);
    git(&project_path, &["commit", "-qm", "init"]);

    let host_lib: Arc<std::sync::OnceLock<host::plugins::hostlib::HostLibHandler>> =
        Default::default();
    let (engine, client, notes) = engine::Embedded::start(engine::EngineConfig {
        data_root: home.path().to_path_buf(),
        project: project_path.clone(),
        groups_file: home.path().join("groups.json"),
        settings: host::hub::cli_settings(),
        model: None,
        host_lib: host_lib.clone(),
    })
    .await
    .unwrap();
    let hub = Hub::start(
        Arc::new(client.clone()),
        notes,
        HubConfig {
            instance: "t".into(),
            host: "h".into(),
            project: project_path.clone(),
            version: "0".into(),
            started_ms: 0,
            registry: Registry::in_memory(),
            jobs: None,
            clock: Arc::new(|| 0),
            engine_model: engine.model().to_string(),
        },
    )
    .await
    .unwrap();
    hub.set_plugin_dirs(vec![store.path().to_path_buf()]);
    let lib_hub = hub.clone();
    let _ = host_lib.set(Arc::new(move |call| {
        Box::pin(host::plugins::hostlib::handle(lib_hub.clone(), call))
    }));
    let flows = host::flows::start(
        &hub,
        host::flows::FlowsConfig {
            runs_dir: home.path().join("runs"),
            worktrees_dir: project_path.join(".wkts"),
            workspaces_dir: home.path().join("ws"),
            policies_file: None,
            command_gates: true,
            artifacts_dir: None,
            library_home: None,
        },
    )
    .await
    .unwrap();

    // 装、披露、启用：两个流程都挂得上（无效的流程会被扣下，不在列表里）。
    let Reply::Plugin { plugin, disclosure } = hub
        .handle(Command::InstallPlugin {
            source: "xforge".into(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(!plugin.enabled);
    assert_eq!(plugin.flows, ["xforge-quick", "xforge-solid"], "{plugin:?}");
    for f in [
        "xforge.open",
        "xforge.check",
        "xforge.close",
        "xforge.command",
    ] {
        assert!(
            disclosure.capabilities.iter().any(|c| c.contains(f)),
            "{f}: {disclosure:?}"
        );
    }
    hub.handle(Command::SetPluginEnabled {
        name: "xforge".into(),
        enabled: true,
    })
    .await
    .unwrap();
    let Reply::Flows(listed) = hub.handle(Command::ListFlows).await.unwrap() else {
        panic!()
    };
    for name in ["xforge-quick", "xforge-solid"] {
        let f = listed
            .iter()
            .find(|f| f.name == name)
            .unwrap_or_else(|| panic!("{name} is withheld: {listed:?}"));
        assert_eq!(f.origin, FlowOrigin::Plugin);
        assert_eq!(f.source.as_deref(), Some("xforge"));
    }

    // 守卫脚本（tool.around）绑得上：包里的绑定兑现不了时只让它自己消失，原因在 script_faults 里。
    let listed = client.call("plugin.list", json!({})).await.unwrap();
    let pkg = listed["plugins"]
        .as_array()
        .and_then(|a| a.iter().find(|p| p["name"] == "xforge"))
        .unwrap_or_else(|| panic!("{listed}"));
    assert!(
        pkg["script_faults"].as_array().is_none_or(|f| f.is_empty()),
        "{pkg}"
    );

    let t = Ctx {
        client: client.clone(),
        project: project_path.clone(),
    };

    // 初始化；测试再把 manifest 换成带两条验证命令的（人写的，不经命令）。
    let r = t
        .cmd(&["xforge", "init", "--verify", "t=true"], None, true)
        .await;
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["data"]["initialised"], true);
    std::fs::write(
        project_path.join("xforge/manifest.yaml"),
        "version: 1\nverification:\n  commands:\n    t: \"true\"\n    nt: \"sh -c 'exit 0'\"\n",
    )
    .unwrap();
    let doctor = t.cmd(&["xforge", "doctor"], None, false).await;
    assert_eq!(doctor["data"]["ok"], true, "{doctor}");

    // 开变更（步骤，可写）：id、模板、台账、基线提交。
    let r = t
        .call(
            "xforge.open",
            step_input(&project_path, "xforge-open", None),
            true,
            json!([]),
        )
        .await;
    assert_eq!(r["ok"], true, "{r}");
    let change = r["value"]["change"].as_str().unwrap().to_string();
    assert!(change.ends_with("-add-widget-support"), "{change}");
    let dir = format!("xforge/changes/{change}");
    assert!(
        t.file(&format!("{dir}/proposal.md"))
            .contains("xforge:template")
    );
    let change_yaml = t.file(&format!("{dir}/change.yaml"));
    assert!(change_yaml.contains("status: open") && change_yaml.contains("baseline_commit"));
    assert!(
        t.file(&format!("{dir}/ledgers/risks.yaml"))
            .contains("entries")
    );

    // propose：提案还是模板、增量是空的，两道门禁都不过。
    let g = t.gate("xforge-structure", "propose", &change).await;
    assert_eq!(g["pass"], false, "{g}");
    assert!(g["detail"].as_str().unwrap().contains("proposal.md"), "{g}");
    let g = t.gate("xforge-spec-delta", "propose", &change).await;
    assert_eq!(g["pass"], false, "{g}");

    // 模型写提案（直接写 .md），增量经命令登记。
    std::fs::write(
        project_path.join(&dir).join("proposal.md"),
        "# 提案\n\n要有 widget。\n",
    )
    .unwrap();
    let added = t
        .ok(
            &[
                "xspec",
                "add",
                "widget",
                "Widgets render",
                "The system renders widgets.",
            ],
            &change,
        )
        .await;
    assert_eq!(added["id"], "widget/R1", "{added}");
    for gate in ["xforge-structure", "xforge-spec-delta"] {
        let g = t.gate(gate, "propose", &change).await;
        assert_eq!(g["pass"], true, "{gate}: {g}");
    }
    let pending = t.ok(&["xspec", "pending"], &change).await;
    assert_eq!(pending[0]["added"][0], "widget/R1", "{pending}");

    // design：工作包要有 paths；原则要在设计里逐条提到；台账里不留 open。
    let g = t.gate("xforge-structure", "design", &change).await;
    assert_eq!(g["pass"], false, "{g}");
    let bad = t
        .cmd(
            &[
                "xflow", "plan", "add", "p0", "x", "--paths", "a", "--verify", "nope",
            ],
            Some(&change),
            true,
        )
        .await;
    assert_eq!(bad["ok"], false, "{bad}");
    t.ok(
        &[
            "xflow",
            "plan",
            "add",
            "p1",
            "Widget core",
            "--paths",
            "src/**",
            "--verify",
            "t",
            "--criteria",
            "renders;no crash",
        ],
        &change,
    )
    .await;
    std::fs::write(
        project_path.join(&dir).join("design.md"),
        "# 设计\n\n做法。\n",
    )
    .unwrap();
    let g = t.gate("xforge-structure", "design", &change).await;
    assert_eq!(g["pass"], true, "{g}");
    std::fs::write(
        project_path.join("xforge/constitution.md"),
        "# 原则\n\n- **P1** keep it small\n- **P2** test it\n",
    )
    .unwrap();
    let g = t.gate("xforge-constitution", "design", &change).await;
    assert_eq!(g["pass"], false, "{g}");
    assert!(g["detail"].as_str().unwrap().contains("P1, P2"), "{g}");
    std::fs::write(
        project_path.join(&dir).join("design.md"),
        "# 设计\n\nP1 小；P2 有测试。\n",
    )
    .unwrap();
    let g = t.gate("xforge-constitution", "design", &change).await;
    assert_eq!(g["pass"], true, "{g}");
    let risk = t
        .ok(
            &[
                "xflow",
                "ledger",
                "add",
                "risks",
                "may be slow",
                "--refs",
                "p1",
            ],
            &change,
        )
        .await;
    assert_eq!(risk["id"], "K1");
    let g = t.gate("xforge-ledgers", "design", &change).await;
    assert_eq!(g["pass"], false, "{g}");
    t.ok(
        &["xflow", "ledger", "set", "risks", "K1", "accepted"],
        &change,
    )
    .await;
    let g = t.gate("xforge-ledgers", "design", &change).await;
    assert_eq!(g["pass"], true, "{g}");

    // packages（步骤）：只列没完成的。
    let r = t
        .call(
            "xforge.packages",
            step_input(&project_path, "xforge-packages", Some(&change)),
            true,
            json!([]),
        )
        .await;
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["value"]["items"][0]["package"], "p1", "{r}");
    assert_eq!(r["value"]["items"][0]["criteria"][1], "no crash", "{r}");

    // apply：integrated、unclaimed、unit-tests。
    let g = t.gate("xforge-integrated", "apply", &change).await;
    assert_eq!(g["pass"], false, "{g}");
    std::fs::create_dir_all(project_path.join("src")).unwrap();
    std::fs::write(project_path.join("src/widget.txt"), "w\n").unwrap();
    let g = t.gate("xforge-unclaimed", "apply", &change).await;
    assert_eq!(g["pass"], true, "{g}");
    std::fs::write(project_path.join("stray.txt"), "s\n").unwrap();
    let g = t.gate("xforge-unclaimed", "apply", &change).await;
    assert_eq!(g["pass"], false, "{g}");
    assert!(g["detail"].as_str().unwrap().contains("stray.txt"), "{g}");
    std::fs::remove_file(project_path.join("stray.txt")).unwrap();

    // `true` 是只读命令：只读的门禁调用不要规则就能跑。
    let g = t.gate("xforge-unit-tests", "apply", &change).await;
    assert_eq!(g["pass"], true, "{g}");
    // 会改东西的命令：只读调用一律不行，要说清是这个原因；让写但没规则，要说明给哪条规则；有规则才过。
    t.ok(
        &[
            "xflow", "plan", "add", "p2", "Second", "--paths", "src/**", "--verify", "nt",
        ],
        &change,
    )
    .await;
    let g = t.gate("xforge-unit-tests", "apply", &change).await;
    assert_eq!(g["pass"], false, "{g}");
    assert!(
        g["detail"].as_str().unwrap().contains("may not write"),
        "{g}"
    );
    let g = t
        .gate_with("xforge-unit-tests", "apply", &change, true, json!([]))
        .await;
    assert_eq!(g["pass"], false, "{g}");
    assert!(
        g["detail"].as_str().unwrap().contains("Bash(sh -c:*)"),
        "{g}"
    );
    let rules = json!([{"source": "session", "behavior": "allow", "tool_name": "Bash",
                        "rule_content": "sh -c:*"}]);
    let g = t
        .gate_with("xforge-unit-tests", "apply", &change, true, rules)
        .await;
    assert_eq!(g["pass"], true, "{g}");
    // 同样的规则给 git status：改动检查带规则也过（本来就是只读命令）。
    let git_rules = json!([{"source": "session", "behavior": "allow", "tool_name": "Bash",
                            "rule_content": "git status:*"}]);
    let g = t
        .gate_with("xforge-unclaimed", "apply", &change, false, git_rules)
        .await;
    assert_eq!(g["pass"], true, "{g}");

    // 只读调用写不了：命令回失败的信封，说明原因，文件没变。
    let before = t.file(&format!("{dir}/work-packages.yaml"));
    let denied = t
        .cmd(
            &["xflow", "plan", "set", "p1", "done", "x"],
            Some(&change),
            false,
        )
        .await;
    assert_eq!(denied["ok"], false, "{denied}");
    assert!(
        denied["error"]["message"]
            .as_str()
            .unwrap()
            .contains("without letting it write"),
        "{denied}"
    );
    assert_eq!(before, t.file(&format!("{dir}/work-packages.yaml")));

    t.ok(&["xflow", "plan", "set", "p1", "done", "built"], &change)
        .await;
    t.ok(&["xflow", "plan", "set", "p2", "done"], &change).await;
    let g = t.gate("xforge-integrated", "apply", &change).await;
    assert_eq!(g["pass"], true, "{g}");

    // verify：验证记录写过才算。
    let g = t.gate("xforge-structure", "verify", &change).await;
    assert_eq!(g["pass"], false, "{g}");
    std::fs::write(
        project_path.join(&dir).join("verification.md"),
        "# 验证\n\n过了。\n",
    )
    .unwrap();
    let g = t.gate("xforge-structure", "verify", &change).await;
    assert_eq!(g["pass"], true, "{g}");
    let shown = t.ok(&["xflow", "show"], &change).await;
    assert_eq!(shown["written"]["verification.md"], true, "{shown}");

    // archive（步骤）：增量并进 specs/，整份变更归档。
    let r = t
        .call(
            "xforge.close",
            step_input(&project_path, "xforge-archive", Some(&change)),
            true,
            json!([]),
        )
        .await;
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["value"]["specs"][0], "widget", "{r}");
    let spec = t.file("xforge/specs/widget.yaml");
    assert!(
        spec.contains("widget/R1") && spec.contains("The system renders widgets."),
        "{spec}"
    );
    let archived = t.file(&format!("xforge/archive/{change}/change.yaml"));
    assert!(archived.contains("status: archived"), "{archived}");
    assert!(
        t.file(&format!("xforge/archive/{change}/proposal.md"))
            .contains("widget")
    );
    // 再调一次是空操作；规格能查到。
    let r = t
        .call(
            "xforge.close",
            step_input(&project_path, "xforge-archive", Some(&change)),
            true,
            json!([]),
        )
        .await;
    assert_eq!(r["ok"], true, "{r}");
    let got = t.cmd(&["xspec", "get", "widget/R1"], None, false).await;
    assert_eq!(got["data"]["title"], "Widgets render", "{got}");
    let doctor = t.cmd(&["xforge", "doctor"], None, false).await;
    assert_eq!(doctor["data"]["ok"], true, "{doctor}");

    // 起一次运行：开变更的程序步骤经 PluginPrograms 调到插件函数（主会话没有模型，走到提案站为止）。
    let Reply::RunStarted(run) = hub
        .handle(Command::StartRun {
            flow: "xforge-quick".into(),
            subject: "second".into(),
            inputs: json!({"title": "Second change", "risk": "low"}),
            step: false,
            session: None,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    let mut opened = false;
    for _ in 0..100 {
        let f = t.cmd(&["xforge", "status"], None, false).await["data"].clone();
        if f.as_array().is_some_and(|a| {
            a.iter()
                .any(|c| c["run"] == run.id.0 && c["title"] == "Second change")
        }) {
            opened = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(opened, "the run's open step did not create the change");

    flows.stop().await;
    engine.shutdown().await;
}
