//! 派工器的会话宿主对着假引擎：键幂等、帧原样分过来并翻对、`run_turn` 的错误也结束那一轮。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use harnix_flow::Delivery;
use harnix_flow::runner::{Frame, OpenSpec, SessionHost, TurnEnd};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::session_host::{ConixSessionHost, session_id_of};
use crate::engine::{Engine, Notification, RpcError};
use crate::hub::{Hub, HubConfig, Registry};

type Handler = Box<dyn Fn(&str, &Value) -> Result<Value, RpcError> + Send + Sync>;

struct Fake {
    calls: Mutex<Vec<(String, Value)>>,
    handler: Handler,
}

#[async_trait]
impl Engine for Fake {
    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_string(), params.clone()));
        (self.handler)(method, &params)
    }
}

const PROJECT: &str = "/work/app";

fn handler(created: Arc<Mutex<Vec<String>>>) -> Handler {
    Box::new(move |method, p| {
        Ok(match method {
            "daemon.ping" => json!({"version": "0.4.0", "protocol_version": 5,
                "capabilities": ["session_settings", "config_explain"]}),
            "session.list" => json!({"sessions": []}),
            "project.status" => json!({"trust": "trusted"}),
            "session.create" => {
                let id = p["session_id"].as_str().unwrap().to_string();
                let mut c = created.lock().unwrap();
                let existing = c.contains(&id);
                c.push(id.clone());
                json!({"session_id": id, "scene": "code", "existing": existing,
                       "created_at": "2026-11-17T10:00:00Z"})
            }
            "session.subscribe" => json!({"session_id": p["session_id"], "last_seq": 0,
                "pending_prompts": [], "pending_host_calls": [], "resumed": false}),
            "session.run_turn" if p["message"] == "fail" => {
                return Err(RpcError::Remote {
                    code: -32000,
                    message: "no_credentials: none".into(),
                    data: json!({"kind": "no_credentials", "retryable": false}),
                });
            }
            "session.run_turn" => json!({"session_id": p["session_id"], "turn_id": p["turn_id"]}),
            _ => json!({}),
        })
    })
}

struct Rig {
    hub: Hub,
    fake: Arc<Fake>,
    notes: mpsc::UnboundedSender<Notification>,
}

async fn rig() -> Rig {
    rig_in(PROJECT).await
}

/// 一个项目，里面放着流程库自带的示例流程（`example-review` 与一条两站的）：没有内置层了，项目跑的只有项目里的。
fn example_project(dir: &std::path::Path) -> std::path::PathBuf {
    let project = dir.join("p");
    let flows = project.join(".conix/flows");
    for (rel, text) in [
        (
            "example-review.yaml",
            include_str!("../../../flow/packs/examples/flows/example-review.yaml"),
        ),
        (
            "skills/review/SKILL.md",
            include_str!("../../../flow/packs/examples/skills/review/SKILL.md"),
        ),
        (
            "rules/review-honest.md",
            include_str!("../../../flow/packs/examples/rules/review-honest.md"),
        ),
        (
            "policies/read-only.yaml",
            include_str!("../../../flow/packs/examples/policies/read-only.yaml"),
        ),
        (
            "two-step.yaml",
            "name: two-step\nstations:\n  - {id: a, exit: [{kind: confirm}]}\n",
        ),
    ] {
        let p = flows.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    project
}

async fn rig_in(project: &str) -> Rig {
    let fake = Arc::new(Fake {
        calls: Mutex::default(),
        handler: handler(Arc::default()),
    });
    let (notes, rx) = mpsc::unbounded_channel();
    let hub = Hub::start(
        fake.clone(),
        rx,
        HubConfig {
            instance: "app".into(),
            host: "box".into(),
            project: project.into(),
            version: "0".into(),
            started_ms: 1,
            registry: Registry::in_memory(),
            jobs: None,
            clock: Arc::new(|| 1_000_000),
            engine_model: "test-model".into(),
        },
    )
    .await
    .unwrap();
    Rig { hub, fake, notes }
}

impl Rig {
    fn calls(&self, method: &str) -> Vec<Value> {
        self.fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
            .collect()
    }

    fn frame(&self, sid: &str, turn: &str, ev: Value) {
        self.notes
            .send(Notification::Session {
                session_id: sid.into(),
                turn_id: Some(turn.into()),
                event: ev,
            })
            .unwrap();
    }
}

fn spec(key: &str) -> OpenSpec {
    let mut s = OpenSpec::new(key, "r1", "code", Delivery::FlowSubmit);
    s.main = key.ends_with("@main");
    s.project_root = Some(PROJECT.into());
    s
}

#[tokio::test]
async fn a_key_opens_the_same_session_and_another_spec_for_it_is_refused() {
    let r = rig().await;
    let host = ConixSessionHost::new(
        r.hub.downgrade(),
        std::sync::Arc::new(super::policy::RunPolicies::in_memory()),
    );
    let a = host.open(spec("flow:r1@main")).await.unwrap();
    assert_eq!(a.id, session_id_of("flow:r1@main"));
    assert!(!a.existed);
    let again = host.open(spec("flow:r1@main")).await.unwrap();
    assert_eq!(again.id, a.id);
    assert!(again.existed, "引擎答的是那个会话");
    let create = &r.calls("session.create")[0];
    assert_eq!(create["session_id"], a.id);
    assert_eq!(create["project_root"], PROJECT);
    assert_eq!(
        create["options"]["settings"]["permissions"]["mode"],
        "acceptEdits"
    );
    let mut other = spec("flow:r1@main");
    other.profile = "plan".into();
    assert!(host.open(other).await.is_err(), "同一个键换了 spec");
    // 主会话的归属是流程运行。
    let (running, project) = r.hub.session_state(&a.id).unwrap();
    assert!(!running);
    assert_eq!(project.as_deref(), Some(PROJECT));
}

/// 站指定了模型（`executor.model`）：会话上记的就是它，不是实例的缺省——流程页的树、报告都按它写。
#[tokio::test]
async fn a_station_session_keeps_the_model_it_was_opened_with() {
    let r = rig().await;
    let host = ConixSessionHost::new(
        r.hub.downgrade(),
        std::sync::Arc::new(super::policy::RunPolicies::in_memory()),
    );
    let mut s = spec("flow:r1/review/1/t");
    s.placement.model = Some("claude-sonnet-5-5".into());
    s.placement.provider = Some("claude".into());
    let o = host.open(s).await.unwrap();
    let create = r.calls("session.create").last().cloned().unwrap();
    assert_eq!(create["options"]["provider"], "claude");
    use crate::service::Service;
    let Ok(port::Reply::Sessions(all)) = r
        .hub
        .handle(port::Command::ListSessions {
            filter: Default::default(),
        })
        .await
    else {
        panic!()
    };
    let sum = all.iter().find(|x| x.id.0 == o.id).expect("会话在列表里");
    assert_eq!(sum.provenance.model, "claude-sonnet-5-5");
}

#[tokio::test]
async fn frames_reach_the_runner_with_the_text_that_started_the_turn() {
    let r = rig().await;
    let host = ConixSessionHost::new(
        r.hub.downgrade(),
        std::sync::Arc::new(super::policy::RunPolicies::in_memory()),
    );
    let id = host.open(spec("flow:r1@main")).await.unwrap().id;
    let mut w = host.watch(&id).await.unwrap();
    host.send(&id, "做这一站").await.unwrap();
    // 等 run_turn 发出去，拿到它的轮 id。
    let mut turn = None;
    for _ in 0..50 {
        tokio::task::yield_now().await;
        if let Some(c) = r.calls("session.run_turn").first() {
            turn = c["turn_id"].as_str().map(str::to_string);
            break;
        }
    }
    let turn = turn.expect("run_turn 发了");
    r.frame(&id, &turn, json!({"kind": "turn_start"}));
    r.frame(
        &id,
        &turn,
        json!({"kind": "tool_use", "id": "c1", "name": "flow_submit", "input": {}}),
    );
    r.frame(
        &id,
        &turn,
        json!({"kind": "turn_complete", "stop_reason": "end_turn", "usage": {"output_tokens": 2}}),
    );
    let f: Vec<Frame> = w.frames.by_ref().take(3).collect().await;
    assert_eq!(
        f[0],
        Frame::TurnStarted {
            turn: turn.clone(),
            text: "做这一站".into()
        }
    );
    assert!(matches!(&f[1], Frame::ToolCall { tool, .. } if tool == "flow_submit"));
    assert!(matches!(
        &f[2],
        Frame::TurnSettled {
            end: TurnEnd::Done,
            ..
        }
    ));

    // run_turn 回错：这一轮照样开始、结束，派工器认得是自己发的那句。
    host.send(&id, "fail").await.unwrap();
    let f: Vec<Frame> = w.frames.by_ref().take(2).collect().await;
    assert!(matches!(&f[0], Frame::TurnStarted { text, .. } if text == "fail"));
    assert!(
        matches!(&f[1], Frame::TurnSettled { end: TurnEnd::Error { retryable: false, kind, .. }, .. } if kind == "no_credentials")
    );
}

#[tokio::test]
async fn a_closed_session_is_gone_and_its_stream_ends() {
    let r = rig().await;
    let host = ConixSessionHost::new(
        r.hub.downgrade(),
        std::sync::Arc::new(super::policy::RunPolicies::in_memory()),
    );
    let id = host.open(spec("flow:r1/s/t@x")).await.unwrap().id;
    let mut w = host.watch(&id).await.unwrap();
    host.close(&id).await.unwrap();
    assert!(w.frames.next().await.is_none(), "看着它的流结束了");
    assert!(host.send(&id, "x").await.is_err());
    assert!(host.lookup(&id).await.unwrap().is_none());
}

/// 流程作业的流程：`#n` 按流程列表认成名字，名字照认，不在的说清楚，inputs 要合流程的 schema。
#[tokio::test]
async fn a_flow_jobs_flow_is_resolved_and_its_inputs_checked() {
    let dir = tempfile::tempdir().unwrap();
    let project = example_project(dir.path());
    let r = rig_in(project.to_str().unwrap()).await;
    let flows = super::start(
        &r.hub,
        super::FlowsConfig {
            runs_dir: dir.path().join("runs"),
            worktrees_dir: dir.path().join(".wkts"),
            workspaces_dir: dir.path().join("ws"),
            policies_file: None,
            command_gates: true,
            artifacts_dir: None,
            library_home: None,
        },
    )
    .await
    .unwrap();
    let all = flows.summaries().await;
    let first = port::order::flows(&all)[0].name.clone();
    use crate::hub::FlowPort;
    assert_eq!(
        flows.check_job_flow("#1", &Value::Null).await.unwrap(),
        first
    );
    assert_eq!(
        flows.check_job_flow(&first, &json!({})).await.unwrap(),
        first
    );
    assert!(
        flows
            .check_job_flow("no-such-flow", &Value::Null)
            .await
            .is_err()
    );
    assert!(
        flows.check_job_flow(&first, &json!(5)).await.is_err(),
        "inputs 不是对象，不合 schema"
    );
    flows.stop().await;
}

/// 根入口不分流程列出运行；已结束的照原样重跑（architecture §8.12）。
#[tokio::test]
async fn project_runs_span_flows_and_an_ended_run_is_run_again() {
    let dir = tempfile::tempdir().unwrap();
    let project = example_project(dir.path());
    let r = rig_in(project.to_str().unwrap()).await;
    let flows = super::start(
        &r.hub,
        super::FlowsConfig {
            runs_dir: dir.path().join("runs"),
            worktrees_dir: dir.path().join(".wkts"),
            workspaces_dir: dir.path().join("ws"),
            policies_file: None,
            command_gates: true,
            artifacts_dir: None,
            library_home: None,
        },
    )
    .await
    .unwrap();
    use crate::hub::FlowPort;
    let all = flows.summaries().await;
    let names: Vec<String> = port::order::flows(&all)
        .iter()
        .map(|f| f.name.clone())
        .collect();
    let a = flows
        .start_run(&names[0], "甲", json!({}), None)
        .await
        .unwrap();
    let b = flows
        .start_run(names.last().unwrap(), "乙", json!({}), None)
        .await
        .unwrap();
    let listed = flows.project_runs(10).await.unwrap();
    for id in [&a.id, &b.id] {
        assert!(listed.iter().any(|x| &x.id == id), "{listed:?}");
    }
    assert!(flows.rerun(&a.id.0).await.is_err(), "没结束的不重跑");
    flows.op(&a.id.0, port::RunOp::Cancel).await.unwrap();
    // 结束了：运行时目录里有 Conix 的报告。
    let report = client::ProjectState::new(&project)
        .run_dir(&a.id.0)
        .join("conix-report.json");
    for _ in 0..200 {
        if report.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let rep: Value = serde_json::from_str(&std::fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(rep["run"]["status"], "cancelled", "{rep}");
    assert!(rep["main_session"]["id"].is_string());
    let again = flows.rerun(&a.id.0).await.unwrap();
    assert_ne!(again.id, a.id);
    assert_eq!(
        (again.flow.as_str(), again.subject.as_str()),
        (a.flow.as_str(), "甲")
    );
    let listed = flows.project_runs(10).await.unwrap();
    assert!(
        listed
            .iter()
            .any(|x| x.id == a.id && x.status == port::RunStatus::Cancelled),
        "已结束的也列出来：{listed:?}"
    );
    flows.stop().await;
}

/// 随 Conix 的 xforge 插件，照 Conix 挂插件的样子挂上：两条流程与它们用到的每道门禁都合法——声明的 `check` 由流程库判，
/// 要插件判的几道在 `[x-conix.gates]` 里登记了。
#[test]
fn the_shipped_xforge_flows_and_gates_hold_together() {
    use harnix_flow::{Packs, PartKind, part_problems, validate};
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/xforge");
    let section = crate::plugins::conix_section(&root).expect("xforge 的 [x-conix]");
    let mounted = [crate::plugins::Mounted {
        name: "xforge".into(),
        root,
        section,
    }];
    let map = crate::plugins::PluginMap::build(&mounted);
    let packs = super::build_packs(&mounted, &map).with_catalog(harnix_flow::Catalog {
        scenes: [super::SCENE.to_string()].into(),
        gates: map.gates.keys().cloned().collect(),
        steps: map.steps.keys().cloned().collect(),
        submit_scenes: [super::SCENE.to_string()].into(),
        main_scene: Some(super::SCENE.into()),
        ..Default::default()
    });
    for f in ["xforge-quick", "xforge-solid"] {
        let (def, _) = packs.flow(f).unwrap_or_else(|| panic!("没有 {f}"));
        assert_eq!(validate(&def, &packs), vec![], "{f}");
        for st in &def.stations {
            for e in &st.exit {
                if let harnix_flow::ExitCond::Gate { gate, .. } = e {
                    let (text, _) = packs.part(PartKind::Gate, gate).unwrap();
                    assert_eq!(
                        part_problems(PartKind::Gate, gate, &text, &packs),
                        Vec::<String>::new(),
                        "{gate}"
                    );
                    let g = packs.gate(gate).unwrap();
                    if let Some(fun) = g.host_function() {
                        assert!(
                            map.gates.contains_key(fun),
                            "{gate} 要插件判，却没登记 {fun}"
                        );
                    }
                }
            }
        }
    }
}

// ── 流程包：目录、命令、站操作、选路、换路、票数 ────────────────────────────────

const BUNDLE_YAML: &str = "name: tb
title: Test bundle
flows: [quick, solid]
start: {route: [quick, solid], triage: judge}
pace: auto
commands:
  note: {read_only: [list], description: keeps notes}
";

/// a 要两票确认，用来看票数。
const QUICK_YAML: &str = "name: quick
title: Quick
inputs:
  risk: {schema: {enum: [low]}}
stations:
  - {id: a, title: A, exit: [{kind: confirm, min: 2, distinct: false}]}
  - {id: b, title: B, needs: [a], executor: {kind: session}, skill: judge, output: {type: object}, exit: [{kind: output}]}
";

/// a 等确认；b 进站搁置，可以打回 a；c 在后面。
const SOLID_YAML: &str = "name: solid
title: Solid
inputs:
  risk: {schema: {enum: [low, medium]}}
stations:
  - {id: a, title: A, exit: [{kind: confirm}]}
  - {id: b, title: B, needs: [a], hold: true, rework_to: [a], executor: {kind: session}, skill: judge, output: {type: object}, exit: [{kind: output}]}
  - {id: c, title: C, needs: [b], executor: {kind: session}, skill: judge, output: {type: object}, exit: [{kind: output}]}
";

const NOTE_JS: &str = "export default function (input, flow) {
  const [sub, ...rest] = input.args;
  if (sub === \"list\") return \"no notes\";
  flow.write.text(`${input.data_dir}/notes.txt`, rest.join(\" \"));
  return \"noted\";
}
";

struct BundleRig {
    r: Rig,
    flows: Arc<super::Flows>,
    _dir: tempfile::TempDir,
}

async fn bundle_rig() -> BundleRig {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("p");
    let b = project.join(".conix/flows/tb");
    std::fs::create_dir_all(b.join("commands")).unwrap();
    std::fs::create_dir_all(b.join("skills/judge")).unwrap();
    // 命令写数据目录，目录要先在（运行起来时库会建；这里没有运行）。
    std::fs::create_dir_all(project.join("conix/tb")).unwrap();
    for (f, t) in [
        ("bundle.yaml", BUNDLE_YAML),
        ("quick.yaml", QUICK_YAML),
        ("solid.yaml", SOLID_YAML),
        ("commands/note.js", NOTE_JS),
        (
            "skills/judge/SKILL.md",
            "---\nname: judge\ndescription: pick the flow\n---\n\nMEDIUM RISK MEANS SOLID.\n",
        ),
    ] {
        std::fs::write(b.join(f), t).unwrap();
    }
    let r = rig_in(project.to_str().unwrap()).await;
    let flows = super::start(
        &r.hub,
        super::FlowsConfig {
            runs_dir: dir.path().join("runs"),
            worktrees_dir: dir.path().join(".wkts"),
            workspaces_dir: dir.path().join("ws"),
            policies_file: None,
            command_gates: true,
            artifacts_dir: Some("conix".into()),
            library_home: None,
        },
    )
    .await
    .unwrap();
    BundleRig {
        r,
        flows,
        _dir: dir,
    }
}

impl BundleRig {
    async fn cmd(&self, c: port::Command) -> Result<port::Reply, port::PortError> {
        use crate::hub::FlowPort;
        self.flows.command(c).await
    }

    async fn start(&self, flow: &str, inputs: Value, step: bool) -> port::RunView {
        let port::Reply::RunStarted(s) = self
            .cmd(port::Command::StartRun {
                flow: flow.into(),
                subject: "s".into(),
                inputs,
                step,
                session: None,
            })
            .await
            .unwrap()
        else {
            panic!("not started")
        };
        self.flows.view(&s.id.0).await.unwrap()
    }

    async fn op(&self, run: &port::RunId, op: port::RunOp) -> Result<port::Reply, port::PortError> {
        self.cmd(port::Command::RunOp {
            run: run.clone(),
            op,
        })
        .await
    }

    async fn text(&self, run: &port::RunId, station: &str) -> Result<String, port::PortError> {
        match self
            .op(
                run,
                port::RunOp::Goto {
                    station: port::StationId(station.into()),
                    reason: None,
                },
            )
            .await?
        {
            port::Reply::Text { text } => Ok(text),
            other => panic!("{other:?}"),
        }
    }

    /// 等运行里的一站到某个状态。
    async fn until(&self, run: &port::RunId, station: &str, want: port::StationStatus) {
        for _ in 0..300 {
            let v = self.flows.view(&run.0).await.unwrap();
            if v.stations
                .iter()
                .any(|s| s.id.0 == station && s.status == want)
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!("station {station} never became {want:?}");
    }

    async fn confirm(
        &self,
        run: &port::RunId,
        station: &str,
    ) -> Result<port::Reply, port::PortError> {
        self.op(
            run,
            port::RunOp::Confirm {
                station: port::StationId(station.into()),
                reject: false,
                reason: None,
            },
        )
        .await
    }
}

#[tokio::test]
async fn the_bundle_directory_lists_flows_stations_commands_and_how_to_start() {
    let t = bundle_rig().await;
    let port::Reply::Bundles(b) = t.cmd(port::Command::ListBundles).await.unwrap() else {
        panic!()
    };
    assert_eq!(b.len(), 1, "{b:?}");
    let tb = &b[0];
    assert_eq!((tb.name.as_str(), tb.title.as_str()), ("tb", "Test bundle"));
    assert_eq!(tb.flows, ["quick", "solid"], "bundle.yaml 的顺序");
    let ids: Vec<&str> = tb.stations.iter().map(|s| s.0.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"], "各流程的站去重");
    assert!(tb.triage);
    assert_eq!(tb.pace, port::Pace::Auto);
    assert_eq!(tb.commands.len(), 1);
    assert_eq!(tb.commands[0].name, "note");
    assert_eq!(tb.commands[0].read_only, ["list"]);
    assert_eq!(tb.commands[0].description, "keeps notes");
    let port::Reply::Flows(f) = t.cmd(port::Command::ListFlows).await.unwrap() else {
        panic!()
    };
    let solid = f.iter().find(|f| f.name == "tb/solid").unwrap();
    assert_eq!(solid.bundle.as_deref(), Some("tb"));
    t.flows.stop().await;
}

#[tokio::test]
async fn a_person_runs_a_writing_command_outside_a_run_and_a_model_only_a_reading_one() {
    let t = bundle_rig().await;
    let c = |args: &[&str]| port::Command::FlowCommand {
        bundle: "tb".into(),
        command: "note".into(),
        args: args.iter().map(|a| a.to_string()).collect(),
        run: None,
    };
    let port::Reply::Text { text } = t.cmd(c(&["add", "hi"])).await.unwrap() else {
        panic!()
    };
    assert!(text.contains("noted"), "{text}");
    let port::Reply::Text { text } = t.cmd(c(&["list"])).await.unwrap() else {
        panic!()
    };
    assert!(text.contains("no notes"), "{text}");
    assert!(
        t.cmd(port::Command::FlowCommand {
            bundle: "tb".into(),
            command: "nope".into(),
            args: vec![],
            run: None,
        })
        .await
        .is_err()
    );
    // 模型（主会话）经工具：读的可以，写的库拒绝。
    let call = |args: Value| {
        let flows = t.flows.clone();
        async move {
            super::tools::call(
                &flows,
                "sess",
                &crate::hub::tools::Scope::Full,
                "flow_command",
                &json!({"command": "tb:note", "args": args}),
            )
            .await
            .expect("routed")
        }
    };
    assert!(call(json!(["list"])).await.unwrap().contains("no notes"));
    let denied = call(json!(["add", "x"])).await.unwrap_err();
    assert!(denied.contains("writes"), "{denied}");
    t.flows.stop().await;
}

#[test]
fn the_main_conversation_gets_flow_command_once() {
    let d = super::tools::declarations();
    assert_eq!(d.iter().filter(|t| t["name"] == "flow_command").count(), 1);
    // 运行里的会话自己的工具列表不再带它（连接上的那份就是它的）。
    let mut s = spec("flow:r@main");
    s.tools = harnix_flow::runner::tool_specs();
    let o = super::session_host::options_of(&s, None);
    assert!(
        !o["host_tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "flow_command")
    );
}

#[tokio::test]
async fn a_station_op_does_what_the_runs_state_and_the_station_call_for() {
    let t = bundle_rig().await;
    let v = t.start("tb/solid", json!({"risk": "low"}), false).await;
    assert_eq!(v.summary.bundle.as_deref(), Some("tb"));
    let id = v.summary.id.clone();
    t.until(&id, "a", port::StationStatus::Working).await;
    // 当前站在等确认：提示，不动。
    let text = t.text(&id, "a").await.unwrap();
    assert!(text.contains("/confirm"), "{text}");
    // 后面的站不跳：说当前站在等什么。
    let text = t.text(&id, "c").await.unwrap();
    assert!(
        text.contains("not reached") && text.contains("waits for a confirmation"),
        "{text}"
    );
    assert!(t.text(&id, "zzz").await.is_err());

    t.confirm(&id, "a").await.unwrap();
    t.until(&id, "b", port::StationStatus::Held).await;
    // 前面过了的站：打回，站又回到等确认。
    let text = t.text(&id, "a").await.unwrap();
    assert!(text.contains("sent the run back"), "{text}");
    t.until(&id, "a", port::StationStatus::Working).await;
    t.confirm(&id, "a").await.unwrap();
    t.until(&id, "b", port::StationStatus::Held).await;
    // 后面的站在搁置的当前站之后：拒绝，并说先放行。
    let text = t.text(&id, "c").await.unwrap();
    assert!(
        text.contains("is held") && text.contains("/release"),
        "{text}"
    );
    // 搁置的当前站：放行。
    let text = t.text(&id, "b").await.unwrap();
    assert!(text.contains("released station b"), "{text}");
    t.until(&id, "b", port::StationStatus::Working).await;
    // 在干活：说它的任务与还欠的出口。
    let text = t.text(&id, "b").await.unwrap();
    assert!(text.contains("is working"), "{text}");
    t.flows.stop().await;
}

#[tokio::test]
async fn votes_of_a_confirm_exit_reach_the_station_view() {
    let t = bundle_rig().await;
    let v = t.start("tb/quick", json!({"risk": "low"}), false).await;
    let id = v.summary.id.clone();
    t.until(&id, "a", port::StationStatus::Working).await;
    let station =
        |v: &port::RunView, s: &str| v.stations.iter().find(|x| x.id.0 == s).unwrap().clone();
    let votes = station(&t.flows.view(&id.0).await.unwrap(), "a")
        .votes
        .unwrap();
    assert_eq!((votes.approvals, votes.min), (0, 2));
    t.confirm(&id, "a").await.unwrap();
    let v = t.flows.view(&id.0).await.unwrap();
    let votes = station(&v, "a").votes.unwrap();
    assert_eq!((votes.approvals, votes.rejections, votes.min), (1, 0, 2));
    assert!(station(&v, "b").votes.is_none());
    t.flows.stop().await;
}

#[tokio::test]
async fn a_bundle_name_routes_by_the_inputs_and_step_holds_the_first_station() {
    let t = bundle_rig().await;
    // risk: low 落在 quick（route 的第一个）上；medium quick 不收，落在 solid。
    let v = t.start("tb", json!({"risk": "low"}), false).await;
    assert_eq!(v.summary.flow, "tb/quick");
    assert_eq!(v.summary.pace, port::Pace::Auto);
    let v = t.start("tb", json!({"risk": "medium"}), false).await;
    assert_eq!(v.summary.flow, "tb/solid");
    // 步进：进站就搁置，站操作放行。
    let v = t.start("tb/solid", json!({"risk": "low"}), true).await;
    assert_eq!(v.summary.pace, port::Pace::Step);
    let id = v.summary.id.clone();
    t.until(&id, "a", port::StationStatus::Held).await;
    assert!(
        t.text(&id, "a")
            .await
            .unwrap()
            .contains("released station a")
    );
    t.flows.stop().await;
}

#[tokio::test]
async fn inputs_that_do_not_decide_are_triaged_in_the_given_chat_and_refused_without_one() {
    let t = bundle_rig().await;
    let start = |session: Option<port::SessionId>| port::Command::StartRun {
        flow: "tb".into(),
        subject: "fix the thing".into(),
        inputs: json!({}),
        step: false,
        session,
    };
    let e = t.cmd(start(None)).await.unwrap_err().to_string();
    assert!(e.contains("chat"), "{e}");
    assert!(
        t.r.calls("session.run_turn").is_empty(),
        "没有对话就什么都不发"
    );

    let host = ConixSessionHost::new(
        t.r.hub.downgrade(),
        Arc::new(super::policy::RunPolicies::in_memory()),
    );
    let chat = host.open(spec("flow:chat@main")).await.unwrap().id;
    let port::Reply::Triage { session } = t
        .cmd(start(Some(port::SessionId(chat.clone()))))
        .await
        .unwrap()
    else {
        panic!("triage expected")
    };
    assert_eq!(session.0, chat);
    // 一轮是另起任务发的。
    for _ in 0..200 {
        if !t.r.calls("session.run_turn").is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let turns = t.r.calls("session.run_turn");
    let msg = turns[0]["message"].as_str().unwrap();
    for needle in [
        "fix the thing",
        "tb/quick",
        "tb/solid",
        "MEDIUM RISK MEANS SOLID",
        "flow_start",
    ] {
        assert!(msg.contains(needle), "{needle} not in {msg}");
    }
    // 分诊没有起运行。
    let port::Reply::Runs(runs) = t
        .cmd(port::Command::ListRuns {
            flow: "tb/quick".into(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(runs.is_empty());
    t.flows.stop().await;
}

#[tokio::test]
async fn a_run_switches_to_another_flow_of_its_bundle_while_its_first_station_is_open() {
    let t = bundle_rig().await;
    let v = t.start("tb/solid", json!({"risk": "low"}), false).await;
    let id = v.summary.id.clone();
    t.until(&id, "a", port::StationStatus::Working).await;
    let port::Reply::Text { text } = t
        .op(
            &id,
            port::RunOp::Switch {
                to: "quick".into(),
                reason: Some("small change".into()),
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(text.contains("tb/quick"), "{text}");
    let v = t.flows.view(&id.0).await.unwrap();
    assert_eq!(v.summary.flow, "tb/quick");
    assert_eq!(v.summary.bundle.as_deref(), Some("tb"));
    // 已经是它了：库拒绝。
    assert!(
        t.op(
            &id,
            port::RunOp::Switch {
                to: "quick".into(),
                reason: Some("again".into())
            }
        )
        .await
        .is_err()
    );
    t.flows.stop().await;
}

/// 全局流程库不参与项目的运行（flows.md §7.12）：库里的 xforge 不在流程列表里，起不了运行，找它时说怎么装；
/// 装进项目、重读之后才有。
#[tokio::test]
async fn the_global_library_is_not_run_until_a_bundle_is_installed() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let project = dir.path().join("p");
    std::fs::create_dir_all(&project).unwrap();
    super::library::Library::new(&home).bootstrap();
    let r = rig_in(project.to_str().unwrap()).await;
    let flows = super::start(
        &r.hub,
        super::FlowsConfig {
            runs_dir: dir.path().join("runs"),
            worktrees_dir: dir.path().join(".wkts"),
            workspaces_dir: dir.path().join("ws"),
            policies_file: None,
            command_gates: true,
            artifacts_dir: Some("conix".into()),
            library_home: Some(home.clone()),
        },
    )
    .await
    .unwrap();
    use crate::hub::FlowPort;
    assert!(
        !flows
            .summaries()
            .await
            .iter()
            .any(|f| f.name.starts_with("xforge/")),
        "no built-in layer"
    );
    let port::Reply::Library(lib) = flows.command(port::Command::ListLibrary).await.unwrap() else {
        panic!("library")
    };
    assert_eq!(lib, ["xforge"]);
    let e = flows
        .start_run("xforge/quick", "s", json!({}), None)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("conix flow install xforge"), "{e}");

    let ctx = super::manage::Ctx::new(&project, &home);
    let plan = super::manage::plan_install(&ctx, "xforge", false).unwrap();
    super::manage::apply(&ctx, &plan).unwrap();
    flows.command(port::Command::ReloadFlows).await.unwrap();
    assert!(
        flows
            .summaries()
            .await
            .iter()
            .any(|f| f.name == "xforge/quick" && f.origin == port::FlowOrigin::Project)
    );
    let port::Reply::Library(lib) = flows.command(port::Command::ListLibrary).await.unwrap() else {
        panic!("library")
    };
    assert!(lib.is_empty(), "{lib:?}");
    flows.stop().await;
}

/// 实例的执行服务接到了派工器上：包的命令在运行里经它起进程，记进运行历史，`enforced` 照实说普通后端什么都没守。
#[tokio::test]
async fn a_bundles_process_goes_through_the_instances_exec_and_is_recorded() {
    let dir = tempfile::tempdir().unwrap();
    let project = example_project(dir.path());
    let b = project.join(".conix/flows/rx");
    for (rel, text) in [
        ("bundle.yaml", "name: rx\ncommands: [report]\n"),
        (
            "work.yaml",
            "name: work\nstations:\n  - {id: a, exit: [{kind: confirm}]}\n",
        ),
        (
            "commands/report.js",
            "export default function (input, flow) { return flow.exec({argv: ['printf', '%s', 'a;b']}); }\n",
        ),
    ] {
        let p = b.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    let r = rig_in(project.to_str().unwrap()).await;
    let flows = super::start(
        &r.hub,
        super::FlowsConfig {
            runs_dir: dir.path().join("runs"),
            worktrees_dir: dir.path().join(".wkts"),
            workspaces_dir: dir.path().join("ws"),
            policies_file: None,
            command_gates: true,
            artifacts_dir: None,
            library_home: None,
        },
    )
    .await
    .unwrap();
    assert!(Arc::ptr_eq(flows.runner.exec(), &crate::exec::service()));
    let run = flows
        .start_run("rx/work", "s", json!({}), None)
        .await
        .unwrap();
    let mut call = harnix_flow::runner::CommandCall::new(
        "rx",
        "report",
        vec!["go".into()],
        harnix_flow::runner::CommandBy::person("me"),
    );
    call.run = Some(run.id.0.clone());
    let reply = flows.runner.run_command(call).await.unwrap();
    assert!(reply.text.contains("\"stdout\": \"a;b\""), "{}", reply.text);
    let history = flows.engine.history(&run.id.0).await.unwrap();
    let line = history
        .iter()
        .find(|h| h.op == "exec")
        .expect("an exec line");
    assert_eq!(line.detail["for"], "flow.exec in rx:report");
    assert_eq!(line.detail["profile"]["kind"], "workspace");
    assert_eq!(
        line.detail["enforced"],
        json!({"backend": "plain", "filesystem": false, "network": false})
    );
    flows.stop().await;
}
