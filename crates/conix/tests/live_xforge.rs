//! LIVE：Conix 内置的 xforge 流程包（源头 `examples/xforge/.conix/flows/xforge`）在演示项目 `examples/xforge` 上真跑，
//! 三条路径各一条，端口驱动（测机制，断言细）。演示项目自己带着这个包，拷进临时仓库就是"装在项目里"。
//!
//! 三条线的模型写在治理文件的 `lines` 里（人在 Conix 之外的事，直接写文件）：开发线 Claude，测试线与检视线 DeepSeek。
//! 用例演所有的人：确认、照默认答澄清的问题、照建议打回（都有上限，免得模型绕圈时一直烧钱）。
//!
//! - quick：只有开发线；只有主人验收；不起子运行；主会话在开发线的模型上。
//! - solid：开发线与测试线。规格侧写一次、人批准、封存；两个子运行由 fork 站起、从同一个分叉提交出发、各在 `.wkts/` 下
//!   自己的 worktree 里，禁读名单盖住父运行与兄弟；测试线只写 `tests_blackbox/`；合回父运行的分支，门禁 blackbox 真跑了；
//!   检视点都关着，检视站都跳过；归档在父运行的 worktree 里；人 `/worktree merge` 之后项目才有改动。
//! - major：三条线。缺省的检视点（spec、design、tests-design）真跑、代码、测试、验证的跳过；每条线的会话在配置的模型上，
//!   检视站的子 Agent 在检视线的模型上，两家都花了；检视线至少一次"记发现 → 自动打回 → 作者答复 → 检视线结掉"，台账历史里看得到。
//! - major 的全链条：治理文件里打开 code、tests、verify，这三站真跑。
//! - 检视线配成与开发线同一个模型：major 第一站就 error，一个会话都没开始干活。
//!
//! 约定见 docs/design/xforge.md。没有 Claude 的 key 就跳过。quick 约 10 分钟，solid 20–30 分钟，major 40–60 分钟。

mod live;

use std::path::Path;
use std::time::Duration;

use live::{Answering, Client, Endpoint, Live};
use port::{
    Command, Event, ProposalStatus, Reply, RunId, RunOp, RunStatus, RunView, StationId,
    StationStatus, TaskId, WorktreeEnd,
};
use serde_json::Value;

const RUN: Duration = Duration::from_secs(3600);

fn example() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/xforge"
    ))
}

fn providers(cl: &Endpoint, ds: &Endpoint) -> Value {
    let one = |id: &str, e: &Endpoint| {
        serde_json::json!({
            "id": id, "vendor": "anthropic", "api": "anthropic",
            "base_url": e.base_url, "api_key_env": e.key_env, "models": [e.model],
            "tiers": {"strong": e.model, "balanced": e.model, "fast": e.model},
            "quota": {"day": 1_000_000_000u64},
        })
    };
    serde_json::json!({
        "providers": [one("claude", cl), one("deepseek", ds)],
        "model": {"order": ["claude", "deepseek"]},
    })
}

fn sh(dir: &str, cmd: &str) -> String {
    let o = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "{cmd}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

struct LogOnPanic<'a>(&'a Live);

impl Drop for LogOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("---- host log ----\n{}", self.0.host_log());
        }
    }
}

/// 治理文件里三条线的模型换成这台机器 `.env` 里配的（`lines:` 到下一个空行），`review` 是检视点那一段要改成的样子。
fn governance(to: &str, lines: &str, review: Option<(&str, &str)>) {
    let p = Path::new(to).join(".conix/governance/xforge.yaml");
    let text = std::fs::read_to_string(&p).unwrap();
    let (head, rest) = text.split_once("lines:\n").expect("演示的治理文件有 lines");
    let (_, tail) = rest.split_once("\n\n").expect("lines 之后有空行");
    let mut text = format!("{head}lines:\n{lines}\n\n{tail}");
    if let Some((from, to)) = review {
        assert!(
            text.contains(from),
            "演示的治理文件改了，这里跟着改：{from}"
        );
        text = text.replace(from, to);
    }
    std::fs::write(&p, text).unwrap();
}

fn lines_of(build: &Endpoint, other: &Endpoint) -> String {
    format!(
        "  build: {{model: {}, provider: claude}}\n  tests: {{model: {}, provider: deepseek}}\n  review: {{model: {}}}",
        build.model, other.model, other.model
    )
}

/// 演示项目拷进实例的项目、写好线的模型、提交（运行的 worktree 从 HEAD 建）。
fn project(lv: &Live, lines: &str, review: Option<(&str, &str)>) -> String {
    let to = lv.project_path();
    sh(&to, &format!("cp -R {}/. .", example().display()));
    governance(&to, lines, review);
    sh(
        &to,
        "git add -A && git -c user.name=live -c user.email=live@conix commit -qm xforge",
    );
    assert_eq!(sh(&to, "git status --porcelain"), "", "从干净的工作区开始");
    to
}

async fn start(c: &Client, flow: &str, subject: &str, inputs: Value) -> RunId {
    match c
        .ok(Command::StartRun {
            flow: flow.into(),
            subject: subject.into(),
            inputs,
            step: false,
            session: None,
        })
        .await
    {
        Reply::RunStarted(s) => s.id,
        r => panic!("{r:?}"),
    }
}

/// 每站每道检查的结论，给人看。
fn report(rec: &Value) {
    let Some(stations) = rec["stations"].as_object() else {
        return;
    };
    for (st, r) in stations {
        eprintln!("[{st}] {}", r["status"]);
        for c in r["checks"].as_array().into_iter().flatten() {
            eprintln!(
                "    {} → {} {}",
                c["label"],
                c["status"],
                c["detail"].as_str().unwrap_or_default()
            );
        }
    }
}

/// `dir` 下（含子目录）叫 `name` 的文件。
fn find_files(dir: &Path, name: &str) -> Vec<std::path::PathBuf> {
    let mut out = vec![];
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(find_files(&p, name));
        } else if e.file_name() == name {
            out.push(p);
        }
    }
    out
}

async fn view(c: &Client, id: &RunId) -> RunView {
    match c.ok(Command::GetRun { run: id.clone() }).await {
        Reply::Run(v) => *v,
        r => panic!("get_run 回的是 {r:?}"),
    }
}

/// 一站在等的、还没处理的建议退回的站，在 `allowed` 里才照它，否则退到 `fallback`。
fn proposed(v: &RunView, st: &str, allowed: &[&str], fallback: &str) -> String {
    v.proposals
        .iter()
        .rev()
        .find(|p| p.station.0 == st && p.status == ProposalStatus::Open)
        .map(|p| p.to.0.clone())
        .filter(|t| allowed.contains(&t.as_str()))
        .unwrap_or_else(|| fallback.into())
}

fn ledger_entries(path: &Path) -> Vec<serde_yaml::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_yaml::from_str::<serde_yaml::Value>(&t).ok())
        .and_then(|v| v["entries"].as_sequence().cloned())
        .unwrap_or_default()
}

/// 工作包登记的路径（`work-packages.yaml` 的 `packages[].paths`）。
fn package_paths(dir: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(dir.join("work-packages.yaml"))
        .unwrap_or_else(|e| panic!("{}/work-packages.yaml：{e}", dir.display()));
    let plan: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    plan["packages"]
        .as_sequence()
        .into_iter()
        .flatten()
        .flat_map(|p| p["paths"].as_sequence().cloned().unwrap_or_default())
        .filter_map(|p| p.as_str().map(String::from))
        .collect()
}

/// 运行结束时 Conix 写的报告（architecture §8.12）：主会话与各站的会话、模型、用量。
async fn run_report(lv: &Live, run: &str) -> Value {
    // 演示项目的制品目录叫 xforge（`.conix/conix.yaml`）：过程目录在 `xforge/runs/<运行>/_run`，worktree 里的运行也是。
    let path = std::path::PathBuf::from(
        lv.process_dir(run)
            .to_string_lossy()
            .replace("/conix/runs/", "/xforge/runs/"),
    )
    .join("conix-report.json");
    for _ in 0..200 {
        if let Ok(t) = std::fs::read_to_string(&path)
            && let Ok(v) = serde_json::from_str(&t)
        {
            return v;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("没有报告：{}", path.display())
}

/// 两家都真花了 token。
fn both_spent(lv: &Live) {
    for id in ["deepseek", "claude"] {
        let ledger = lv.home.path().join("quota").join(format!("{id}.json"));
        let text = std::fs::read_to_string(&ledger).unwrap_or_else(|e| {
            panic!("{id} 没有用量记录（没被调用过？）{}: {e}", ledger.display())
        });
        assert!(
            text.chars().any(|c| c.is_ascii_digit() && c != '0'),
            "{id} 花了 token：{text}"
        );
    }
}

/// 演人的那一边：打回与重判都有上限。
#[derive(Default)]
struct People {
    confirmed: Vec<String>,
    clarify_rounds: u32,
    reworks: std::collections::BTreeMap<String, u32>,
    /// 子运行还在跑时看到的它的 worktree（合回去之后就删了）。
    worktrees_seen: std::collections::BTreeSet<String>,
}

impl People {
    fn count(&mut self, key: &str, max: u32) {
        let n = self.reworks.entry(key.to_string()).or_default();
        *n += 1;
        assert!(*n <= max, "{key} 人打回了 {max} 次还没过");
    }
}

async fn rework(c: &Client, run: &RunId, to: &str, tasks: Vec<TaskId>, why: &str) {
    eprintln!("==== {} 打回到 {to} {tasks:?}：{why}", run.0);
    c.ok(Command::RunOp {
        run: run.clone(),
        op: RunOp::Rework {
            to: StationId(to.into()),
            tasks,
            reason: Some(why.into()),
        },
    })
    .await;
}

/// 子运行无人值守：自动打回用完了还没达标，像人那样照建议（没有就退到写它的那一站）再打回一次，至多两次。
async fn tend_child(c: &mut Client, lv: &Live, child: &RunId, p: &mut People) {
    if let Some(rec) = lv.try_state(&format!("flow-runs/{}.json", child.0))
        && let Some(w) = rec["worktree"]["path"].as_str()
        && Path::new(w).is_dir()
    {
        p.worktrees_seen.insert(w.to_string());
    }
    let v = view(c, child).await;
    match &v.summary.status {
        RunStatus::Waiting { reason } if reason == "unmet" => {
            let st = v
                .stations
                .iter()
                .find(|s| s.status == StationStatus::Unmet)
                .map(|s| s.id.0.clone())
                .unwrap_or_default();
            p.count(&format!("{} {st}", child.0), 2);
            let author = if matches!(st.as_str(), "apply" | "review-code" | "review-tests") {
                "apply"
            } else {
                "design"
            };
            let to = proposed(&v, &st, &["design", "apply"], author);
            rework(
                c,
                child,
                &to,
                vec![],
                "没达标的照台账与门禁的说明改，规格侧不动",
            )
            .await;
        }
        RunStatus::Waiting { reason } if ["failed", "held"].contains(&reason.as_str()) => {
            panic!("子运行 {} 停下了：{reason}\n{:#?}", child.0, v.stations)
        }
        RunStatus::Cancelled => panic!("子运行 {} 被取消了", child.0),
        _ => {}
    }
}

/// 把一次运行（连同它的子运行）跑到结束：确认等人的站、答澄清、没达标时照建议打回。
async fn drive(c: &mut Client, lv: &Live, id: &RunId) -> (RunView, People) {
    let rec_of = |run: &str| lv.read_state(&format!("flow-runs/{run}.json"));
    let mut p = People::default();
    let done = loop {
        let at = c
            .wait_event("运行有动静", RUN, |e| match e {
                Event::RunDetail(v)
                    if v.summary.id == *id || v.summary.flow.starts_with("xforge/line-") =>
                {
                    Some(v.summary.id.clone())
                }
                _ => None,
            })
            .await;
        if at != *id {
            tend_child(c, lv, &at, &mut p).await;
            continue;
        }
        // 照此刻的样子判，不照事件里的：事件可能是做了上一步之前发的。
        let v = view(c, id).await;
        let out = rec_of(&id.0)["output_dir"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        match &v.summary.status {
            RunStatus::Finished => break v,
            RunStatus::Cancelled => panic!("运行被取消了：{:#?}", v.stations),
            RunStatus::Waiting { reason } if reason == "confirm" => {
                let st = v
                    .stations
                    .iter()
                    .find(|s| s.status == StationStatus::Working)
                    .unwrap_or_else(|| panic!("等确认却没有在做的站：{:#?}", v.stations))
                    .id
                    .0
                    .clone();
                let why = match st.as_str() {
                    "propose" => "破坏性改动看过了，批准",
                    "clarify" => "范围定了",
                    "seal" => "规格侧定了",
                    "join" => "两路对上了",
                    "verify" | "accept" => "验收通过",
                    _ => "确认",
                };
                eprintln!("==== {st} 等人：{why}");
                report(&rec_of(&id.0));
                c.ok(Command::RunOp {
                    run: id.clone(),
                    op: RunOp::Confirm {
                        station: StationId(st.clone()),
                        reject: false,
                        reason: Some(why.into()),
                    },
                })
                .await;
                p.confirmed.push(st);
            }
            RunStatus::Waiting { reason } if reason == "unmet" => {
                let st = v
                    .stations
                    .iter()
                    .find(|s| s.status == StationStatus::Unmet)
                    .map(|s| s.id.0.clone())
                    .unwrap_or_default();
                eprintln!("==== {st} 没达标");
                report(&rec_of(&id.0));
                match st.as_str() {
                    // 澄清的问题只有人答：每条照它给的默认答，再重判这一站（/xforge:clarify）。
                    "clarify" => {
                        p.clarify_rounds += 1;
                        assert!(
                            p.clarify_rounds <= 3,
                            "答了三轮澄清还没过：{:#?}",
                            v.stations
                        );
                        let qs = ledger_entries(&Path::new(&out).join("ledgers/questions.yaml"));
                        for q in qs
                            .iter()
                            .filter(|q| q["conclusion"].as_str() == Some("open"))
                        {
                            let qid = q["id"].as_str().unwrap().to_string();
                            let text = q["text"].as_str().unwrap_or_default();
                            let answer = text
                                .split_once("默认：")
                                .map(|(_, d)| format!("照默认：{}", d.trim()))
                                .unwrap_or_else(|| "照你建议的做，范围不扩大".into());
                            eprintln!("---- 答 {qid}：{text} → {answer}");
                            c.ok(Command::FlowCommand {
                                bundle: "xforge".into(),
                                command: "xledger".into(),
                                args: vec![
                                    "set".into(),
                                    "questions".into(),
                                    qid,
                                    "answered".into(),
                                    answer,
                                ],
                                run: Some(id.clone()),
                            })
                            .await;
                        }
                        c.ok(Command::RunOp {
                            run: id.clone(),
                            op: RunOp::Goto {
                                station: StationId("clarify".into()),
                                reason: Some("问题答完了".into()),
                            },
                        })
                        .await;
                    }
                    // 黑盒测试没过：错在哪一边由人定，这里照合起来那一站的建议。会话判出错在一路时（交出的
                    // `rework.tasks`），只重做分叉站里那一路，另一路的结果留着；判不出就两路都重新分叉。
                    "join" => {
                        p.count("join", 2);
                        let back = proposed(&v, "join", &["fork", "propose"], "fork");
                        let rec = rec_of(&id.0);
                        let tasks: Vec<TaskId> = (back == "fork")
                            .then(|| {
                                rec["stations"]["join"]["tasks"]
                                    .as_array()
                                    .and_then(|t| t.last())
                                    .and_then(|t| t["output"]["rework"]["tasks"].as_array())
                                    .map(|t| {
                                        t.iter()
                                            .filter_map(|x| x.as_str())
                                            .map(|x| TaskId(x.into()))
                                            .collect()
                                    })
                            })
                            .flatten()
                            .unwrap_or_default();
                        eprintln!(
                            "---- ledgers/findings.yaml\n{}",
                            std::fs::read_to_string(Path::new(&out).join("ledgers/findings.yaml"))
                                .unwrap_or_default()
                        );
                        rework(
                            c,
                            id,
                            &back,
                            tasks,
                            "黑盒测试没过：照 findings 台账按规格修，不照对面凑",
                        )
                        .await;
                    }
                    "lines" => panic!("三条线没核对过：{:#?}", v.stations),
                    // 自动打回用完了还没过：照建议（没有就退到写它的那一站）再打回一次。
                    other => {
                        p.count(other, 2);
                        let author = match other {
                            "review-spec" => "propose",
                            "review-verify" => "verify",
                            "apply" => "apply",
                            s => s,
                        };
                        let back = proposed(
                            &v,
                            other,
                            &["propose", "clarify", "apply", "verify", "fork"],
                            author,
                        );
                        rework(c, id, &back, vec![], "还没达标：照台账与门禁的说明改").await;
                    }
                }
            }
            RunStatus::Waiting { reason } if ["failed", "held"].contains(&reason.as_str()) => {
                panic!("运行停下了：{reason}\n{:#?}", v.stations)
            }
            _ => {}
        }
    };
    let rec = rec_of(&id.0);
    eprintln!(
        "==== 结束（澄清 {} 轮，人打回 {:?}；确认了 {:?}）",
        p.clarify_rounds, p.reworks, p.confirmed
    );
    report(&rec);
    (done, p)
}

/// 每条线的会话在配置的模型上：主会话（开发线、测试线），检视站的子 Agent（检视线）。
async fn models_of(lv: &Live, run: &str, main: &str, review: Option<&str>) {
    let r = run_report(lv, run).await;
    assert_eq!(
        r["main_session"]["model"].as_str(),
        Some(main),
        "{run} 的主会话：{}",
        r["main_session"]
    );
    let agents: Vec<&Value> = r["main_session"]["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| t["label"].as_str().is_some_and(|l| l.contains("/review-")))
        .collect();
    match review {
        Some(m) => {
            assert!(
                !agents.is_empty(),
                "{run} 没有检视站的子 Agent：{}",
                r["main_session"]
            );
            for t in agents {
                assert_eq!(
                    t["model"].as_str(),
                    Some(m),
                    "{run} 的检视在检视线的模型上：{t}"
                );
            }
        }
        None => assert!(agents.is_empty(), "{run} 不该有检视：{agents:?}"),
    }
}

fn child_runs(rec: &Value) -> std::collections::BTreeMap<String, String> {
    rec["stations"]["fork"]["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            (
                t["id"].as_str().unwrap().to_string(),
                t["child"].as_str().expect("任务的子运行").to_string(),
            )
        })
        .collect()
}

fn status_of(rec: &Value, st: &str) -> String {
    rec["stations"][st]["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_builtin_xforge_quick_runs_one_line_with_only_the_owners_acceptance() {
    let Some(cl) = live::claude() else {
        eprintln!("skipped: .env has no CLAUDE_API_KEY / CLAUDE_MODEL");
        return;
    };
    let ds = live::deepseek();
    let lv = Live::with(live::Opts {
        settings: Some(providers(&cl, &ds)),
        ..Default::default()
    });
    let _log = LogOnPanic(&lv);
    let to = project(&lv, &lines_of(&cl, &ds), None);
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let id = start(
        &c,
        "xforge/quick",
        "给 textkit 加 truncate(text, n)：文字超过 n 个字符时截成前 n-1 个字符再加一个省略号 …，不超过时原样返回",
        serde_json::json!({"title": "truncate", "risk": "low"}),
    )
    .await;
    let (done, p) = drive(&mut c, &lv, &id).await;
    for s in &done.stations {
        assert_eq!(s.status, StationStatus::Passed, "{}", s.id.0);
    }
    assert_eq!(p.confirmed, ["verify"], "只有主人验收");
    let rec = lv.read_state(&format!("flow-runs/{}.json", id.0));
    assert!(
        rec["worktree"].is_null(),
        "quick 在项目里跑：{}",
        rec["worktree"]
    );
    let spawned: Vec<Value> = lv
        .run_history(&id.0)
        .into_iter()
        .filter(|h| h["op"] == "spawn")
        .collect();
    assert!(spawned.is_empty(), "quick 不起子运行：{spawned:?}");
    models_of(&lv, &id.0, &cl.model, None).await;

    let spec = std::fs::read_to_string(Path::new(&to).join("xforge/specs/textkit.md")).unwrap();
    assert!(spec.contains("### TK-R3"), "{spec}");
    assert!(
        std::fs::read_to_string(Path::new(&to).join("src/textkit/__init__.py"))
            .unwrap()
            .contains("def truncate")
    );
    eprintln!(
        "---- verify\n{}",
        sh(&to, "sh .conix/governance/verify.sh 2>&1")
    );
    let archived = Path::new(&to).join(rec["archive"]["dir"].as_str().expect("归档了"));
    assert!(
        archived.join("record.json").is_file(),
        "{}",
        archived.display()
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_builtin_xforge_solid_forks_the_build_and_tests_lines_and_joins_them() {
    let Some(cl) = live::claude() else {
        eprintln!("skipped: .env has no CLAUDE_API_KEY / CLAUDE_MODEL");
        return;
    };
    let ds = live::deepseek();
    let lv = Live::with(live::Opts {
        settings: Some(providers(&cl, &ds)),
        ..Default::default()
    });
    let _log = LogOnPanic(&lv);
    let to = project(&lv, &lines_of(&cl, &ds), None);
    let base = sh(&to, "git rev-parse HEAD").trim().to_string();
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let id = start(
        &c,
        "xforge/solid",
        "给 textkit 加 truncate(text, n)：文字超过 n 个字符时截成前 n-1 个字符再加一个省略号 …，不超过时原样返回；n 小于 1 时抛 ValueError",
        serde_json::json!({"title": "truncate", "risk": "medium"}),
    )
    .await;
    let (done, p) = drive(&mut c, &lv, &id).await;
    let rec_of = |run: &str| lv.read_state(&format!("flow-runs/{run}.json"));
    let rec = rec_of(&id.0);
    // 检视点都关着：检视站跳过，别的都过了。
    for s in &done.stations {
        let want = if s.id.0.starts_with("review-") {
            StationStatus::Skipped
        } else {
            StationStatus::Passed
        };
        assert_eq!(s.status, want, "{}", s.id.0);
    }
    for st in ["seal", "join", "accept"] {
        assert!(
            p.confirmed.iter().any(|s| s == st),
            "{st} 由人确认：{:?}",
            p.confirmed
        );
    }
    // 加一个函数不破坏什么：批准破坏性改动没要，记在提案站的 not_required 里，人没被问。
    let left_out = rec["stations"]["propose"]["not_required"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        left_out
            .iter()
            .any(|n| n["label"] == "批准破坏性改动" && n["value"] == false),
        "{left_out:?}"
    );
    assert!(
        !p.confirmed.iter().any(|s| s == "propose"),
        "{:?}",
        p.confirmed
    );

    // 父运行在 .wkts/ 下自己的 worktree 里，运行目录在那里面。
    let wt = rec["worktree"]["path"]
        .as_str()
        .expect("父运行的 worktree")
        .to_string();
    let branch = rec["worktree"]["branch"].as_str().unwrap().to_string();
    assert!(wt.starts_with(&format!("{to}/.wkts/")), "{wt}");
    let out = rec["output_dir"].as_str().unwrap().to_string();

    // 两个子运行从同一个分叉提交出发，各在自己的 worktree 里，互相读不到，合回父运行的分支。
    let commit = rec["stations"]["fork"]["fork"]["commit"]
        .as_str()
        .expect("分叉提交")
        .to_string();
    let children = child_runs(&rec);
    assert_eq!(
        children.keys().collect::<Vec<_>>(),
        ["build", "tests"],
        "{children:?}"
    );
    let crec: std::collections::BTreeMap<&str, Value> = children
        .iter()
        .map(|(t, run)| (t.as_str(), rec_of(run)))
        .collect();
    let path_of = |t: &str| crec[t]["worktree"]["path"].as_str().unwrap().to_string();
    for (task, other) in [("build", "tests"), ("tests", "build")] {
        let r = &crec[task];
        assert_eq!(r["status"], "finished", "{task}");
        assert_eq!(
            r["base_commit"].as_str(),
            Some(commit.as_str()),
            "{task}：同一个分叉提交"
        );
        let mine = path_of(task);
        assert!(
            mine.starts_with(&format!("{to}/.wkts/")) && mine != wt && mine != path_of(other),
            "{task}: {mine}"
        );
        assert!(
            p.worktrees_seen.contains(&mine),
            "{task} 跑的时候它的 worktree 在：{:?}",
            p.worktrees_seen
        );
        let f = &r["worktree"]["fork"];
        assert_eq!(f["parent_path"].as_str(), Some(wt.as_str()), "{task}");
        let siblings: Vec<&str> = f["siblings"]
            .as_array()
            .expect("禁读的兄弟名单")
            .iter()
            .filter_map(|s| s.as_str())
            .collect();
        assert!(
            siblings.contains(&path_of(other).as_str()),
            "{task}: {siblings:?}"
        );
        let end = &r["worktree"]["outcome"];
        assert_eq!(
            (end["kind"].as_str(), end["into"].as_str()),
            (Some("merged"), Some(branch.as_str())),
            "{task}: {end}"
        );
        let back = Path::new(&out).join("children/fork").join(task);
        for f in ["design.md", "work-packages.yaml"] {
            assert!(back.join(f).is_file(), "{}/{f}", back.display());
        }
        for (st, s) in r["stations"].as_object().unwrap() {
            if st.starts_with("review-") {
                assert_eq!(s["status"], "skipped", "{task} {st}");
                continue;
            }
            let checks = s["checks"].as_array().cloned().unwrap_or_default();
            assert!(
                checks.iter().any(|c| c["label"]
                    .as_str()
                    .is_some_and(|l| l.contains("spec-unchanged"))
                    && c["status"] == "pass"),
                "{task} {st}: {checks:#?}"
            );
        }
    }
    // 测试线只写 tests_blackbox/，开发线改 src/、不碰黑盒测试。
    let tests_paths = package_paths(&Path::new(&out).join("children/fork/tests"));
    assert!(
        !tests_paths.is_empty() && tests_paths.iter().all(|p| p.starts_with("tests_blackbox/")),
        "{tests_paths:?}"
    );
    let build_paths = package_paths(&Path::new(&out).join("children/fork/build"));
    assert!(
        build_paths.iter().any(|p| p.starts_with("src/"))
            && !build_paths.iter().any(|p| p.starts_with("tests_blackbox/")),
        "{build_paths:?}"
    );
    let changed = sh(&wt, &format!("git diff --name-only {commit} --"));
    assert!(
        changed.lines().any(|l| l.starts_with("tests_blackbox/"))
            && changed.lines().any(|l| l == "src/textkit/__init__.py"),
        "{changed}"
    );
    // 每条线在自己的模型上：父运行与开发线在开发线的模型上，测试线在测试线的。
    models_of(&lv, &id.0, &cl.model, None).await;
    models_of(&lv, &children["build"], &cl.model, None).await;
    models_of(&lv, &children["tests"], &ds.model, None).await;
    both_spent(&lv);

    // 门禁 blackbox 真跑了黑盒测试，最后一轮过了；它起的进程进了引擎的沙箱，记成运行历史的 exec。
    let logs = find_files(Path::new(&to), "gate-xforge-blackbox.log");
    assert!(
        logs.iter()
            .filter_map(|l| std::fs::read_to_string(l).ok())
            .any(|t| t.lines().any(|l| l.starts_with("== blackbox"))),
        "门禁 blackbox 没跑：{logs:?}"
    );
    let execs: Vec<Value> = lv
        .run_history(&id.0)
        .into_iter()
        .filter(|h| h["op"] == "exec")
        .collect();
    let bb = execs
        .iter()
        .find(|h| {
            h["detail"]["for"]
                .as_str()
                .is_some_and(|f| f.starts_with("gate xforge/blackbox"))
        })
        .unwrap_or_else(|| panic!("没有门禁 blackbox 的 exec 记录：{execs:#?}"));
    assert_eq!(bb["detail"]["enforced"]["backend"], "agix", "{bb}");
    for (task, other) in [("build", "tests"), ("tests", "build")] {
        let gates: Vec<Value> = lv
            .run_history(&children[task])
            .into_iter()
            .filter(|h| h["op"] == "exec")
            .collect();
        assert!(!gates.is_empty(), "{task} 的门禁没起进程");
        for h in &gates {
            let denied: Vec<&str> = h["detail"]["deny_read"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            assert!(
                denied.contains(&wt.as_str()) && denied.contains(&path_of(other).as_str()),
                "{task} 的进程该读不到父运行与 {other}：{h}"
            );
        }
    }

    // 归档在父运行的 worktree 里：两条基线都并进了，两路的设计都留着。
    let spec = std::fs::read_to_string(Path::new(&wt).join("xforge/specs/textkit.md")).unwrap();
    assert!(spec.contains("### TK-R3"), "{spec}");
    let api =
        std::fs::read_to_string(Path::new(&wt).join("xforge/interfaces/textkit.yaml")).unwrap();
    assert!(api.contains("fn:textkit.truncate"), "{api}");
    let archived = Path::new(&wt).join(rec["archive"]["dir"].as_str().expect("归档了"));
    for f in [
        "record.json",
        "proposal.md",
        "verification.md",
        "children/fork/build/design.md",
        "children/fork/tests/design.md",
    ] {
        assert!(archived.join(f).is_file(), "{}/{f}", archived.display());
    }

    // 项目的分支没动，直到人收尾（`/worktree merge`，端口的 `EndWorktree`）。
    assert_eq!(
        sh(&to, "git rev-parse HEAD").trim(),
        base,
        "项目的分支没被自动提交"
    );
    let w = view(&c, &id).await.worktree.expect("父运行有 worktree");
    assert!(w.decidable && w.branch == branch, "{w:?}");
    c.ok(Command::RunOp {
        run: id.clone(),
        op: RunOp::EndWorktree {
            how: WorktreeEnd::Merged,
        },
    })
    .await;
    let w = view(&c, &id).await.worktree.unwrap();
    assert_eq!(
        (w.outcome, w.decidable),
        (Some(WorktreeEnd::Merged), false),
        "{w:?}"
    );
    assert!(!Path::new(&wt).exists(), "合了之后 worktree 删了：{wt}");
    assert!(
        std::fs::read_to_string(Path::new(&to).join("src/textkit/__init__.py"))
            .unwrap()
            .contains("def truncate")
    );
    eprintln!(
        "---- verify\n{}\n---- blackbox\n{}",
        sh(&to, "sh .conix/governance/verify.sh 2>&1"),
        sh(
            &to,
            "python3 -B -m unittest discover -s tests_blackbox -q 2>&1"
        )
    );
}

/// 运行与两路的台账里，检视站记的发现：`(哪份台账, 编号, 历史里每一步 (结论, 谁))`。
/// 一条检视站记的发现：哪份台账、编号、历史里每一步（结论, 谁）。
type Reviewed = (String, String, Vec<(String, String)>);

fn reviewed_findings(out: &Path) -> Vec<Reviewed> {
    let mut found = vec![];
    for dir in ["", "children/fork/build/", "children/fork/tests/"] {
        let ledger = out.join(format!("{dir}ledgers/findings.yaml"));
        for e in ledger_entries(&ledger) {
            let steps: Vec<(String, String)> = e["history"]
                .as_sequence()
                .into_iter()
                .flatten()
                .map(|h| {
                    (
                        h["conclusion"].as_str().unwrap_or_default().to_string(),
                        h["by"]["id"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            if steps
                .first()
                .is_some_and(|(_, by)| by.starts_with("review-"))
            {
                found.push((
                    dir.to_string(),
                    e["id"].as_str().unwrap_or_default().to_string(),
                    steps,
                ));
            }
        }
    }
    found
}

/// 一次 major：返回运行、它的记录、两路的运行。
async fn major_run(
    lv: &Live,
    c: &mut Client,
    subject: &str,
) -> (
    RunId,
    Value,
    std::collections::BTreeMap<String, String>,
    People,
) {
    let id = start(
        c,
        "xforge/major",
        subject,
        serde_json::json!({"title": "truncate", "risk": "high"}),
    )
    .await;
    let (_, p) = drive(c, lv, &id).await;
    let rec = lv.read_state(&format!("flow-runs/{}.json", id.0));
    let children = child_runs(&rec);
    (id, rec, children, p)
}

// 留了一处边界没说：规格侧的检视多半会记下它，打回提案补上。
const MAJOR_SUBJECT: &str =
    "给 textkit 加 truncate(text, n)：文字超过 n 个字符时截短，加一个省略号 …；不超过时原样返回";

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_builtin_xforge_major_reviews_the_design_stages_on_another_model() {
    let Some(cl) = live::claude() else {
        eprintln!("skipped: .env has no CLAUDE_API_KEY / CLAUDE_MODEL");
        return;
    };
    let ds = live::deepseek();
    let lv = Live::with(live::Opts {
        settings: Some(providers(&cl, &ds)),
        ..Default::default()
    });
    let _log = LogOnPanic(&lv);
    project(&lv, &lines_of(&cl, &ds), None);
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let (id, rec, children, p) = major_run(&lv, &mut c, MAJOR_SUBJECT).await;
    let rec_of = |run: &str| lv.read_state(&format!("flow-runs/{run}.json"));
    // 缺省的检视点真跑了，代码、测试、验证的跳过。
    assert_eq!(
        (
            status_of(&rec, "review-spec").as_str(),
            status_of(&rec, "review-verify").as_str()
        ),
        ("passed", "skipped")
    );
    let (b, t) = (rec_of(&children["build"]), rec_of(&children["tests"]));
    assert_eq!(
        (
            status_of(&b, "review-design").as_str(),
            status_of(&b, "review-code").as_str()
        ),
        ("passed", "skipped")
    );
    assert_eq!(
        (
            status_of(&t, "review-tests-design").as_str(),
            status_of(&t, "review-tests").as_str()
        ),
        ("passed", "skipped")
    );
    for st in ["clarify", "seal", "join", "accept"] {
        assert!(
            p.confirmed.iter().any(|s| s == st),
            "{st} 由人确认：{:?}",
            p.confirmed
        );
    }
    // 检视线的来回：检视站记下，作者答复（修好或不同意），同一个检视站再来结掉。
    let out = Path::new(rec["output_dir"].as_str().unwrap()).to_path_buf();
    let found = reviewed_findings(&out);
    eprintln!("---- 检视线的发现\n{found:#?}");
    let looped = found.iter().find(|(_, _, steps)| {
        let by = &steps[0].1;
        let answered = steps.iter().position(|(c, who)| {
            !who.starts_with("review-") && (c == "resolved" || c == "disputed")
        });
        answered.is_some_and(|i| {
            steps[i + 1..]
                .iter()
                .any(|(c, who)| who == by && (c == "resolved" || c == "rejected"))
        })
    });
    assert!(looped.is_some(), "没有一条走完四步的发现：{found:#?}");
    // 每条线在自己的模型上；检视站的子 Agent 在检视线的模型上。
    models_of(&lv, &id.0, &cl.model, Some(&ds.model)).await;
    models_of(&lv, &children["build"], &cl.model, Some(&ds.model)).await;
    models_of(&lv, &children["tests"], &ds.model, Some(&ds.model)).await;
    both_spent(&lv);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_builtin_xforge_major_reviews_the_whole_chain_when_a_person_turns_it_on() {
    let Some(cl) = live::claude() else {
        eprintln!("skipped: .env has no CLAUDE_API_KEY / CLAUDE_MODEL");
        return;
    };
    let ds = live::deepseek();
    let lv = Live::with(live::Opts {
        settings: Some(providers(&cl, &ds)),
        ..Default::default()
    });
    let _log = LogOnPanic(&lv);
    // 人在 Conix 之外打开全链条：改治理文件。
    project(
        &lv,
        &lines_of(&cl, &ds),
        Some((
            "major: {spec: true, design: true, tests-design: true, code: false, tests: false, verify: false}",
            "major: {spec: true, design: true, tests-design: true, code: true, tests: true, verify: true}",
        )),
    );
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let (id, rec, children, _) = major_run(
        &lv,
        &mut c,
        "给 textkit 加 truncate(text, n)：文字超过 n 个字符时截成前 n-1 个字符再加一个省略号 …，不超过时原样返回；n 小于 1 时抛 ValueError",
    )
    .await;
    let rec_of = |run: &str| lv.read_state(&format!("flow-runs/{run}.json"));
    let (b, t) = (rec_of(&children["build"]), rec_of(&children["tests"]));
    for (r, st) in [
        (&rec, "review-spec"),
        (&rec, "review-verify"),
        (&b, "review-design"),
        (&b, "review-code"),
        (&t, "review-tests-design"),
        (&t, "review-tests"),
    ] {
        assert_eq!(status_of(r, st), "passed", "{st} 真跑了");
    }
    models_of(&lv, &id.0, &cl.model, Some(&ds.model)).await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_review_line_on_the_build_lines_model_stops_major_before_any_work() {
    let Some(cl) = live::claude() else {
        eprintln!("skipped: .env has no CLAUDE_API_KEY / CLAUDE_MODEL");
        return;
    };
    let ds = live::deepseek();
    let lv = Live::with(live::Opts {
        settings: Some(providers(&cl, &ds)),
        ..Default::default()
    });
    let _log = LogOnPanic(&lv);
    project(&lv, &lines_of(&cl, &cl), None);
    let mut c = lv.start_trusted().await;
    let id = start(
        &c,
        "xforge/major",
        "加 truncate",
        serde_json::json!({"title": "truncate", "risk": "high"}),
    )
    .await;
    let v = c
        .wait_event("第一站停下", Duration::from_secs(120), |e| match e {
            Event::RunDetail(v) if v.summary.id == id && matches!(&v.summary.status, RunStatus::Waiting { reason } if reason == "unmet") => {
                Some((**v).clone())
            }
            _ => None,
        })
        .await;
    let rec = lv.read_state(&format!("flow-runs/{}.json", id.0));
    report(&rec);
    let lines = v.stations.iter().find(|s| s.id.0 == "lines").unwrap();
    assert_eq!(lines.status, StationStatus::Unmet);
    let check = rec["stations"]["lines"]["checks"][0].clone();
    assert_eq!(check["status"], "error", "{check}");
    assert!(
        check["detail"]
            .as_str()
            .unwrap()
            .contains(&format!("检视线与开发线是同一个模型 {}", cl.model)),
        "{check}"
    );
    assert!(
        !matches!(status_of(&rec, "propose").as_str(), "working" | "passed"),
        "提案站还没进：{}",
        rec["stations"]["propose"]
    );
    // 自检说的是同一件事。
    let doctor = String::from_utf8_lossy(&lv.conix_flow(&["doctor"]).stdout).into_owned();
    assert!(doctor.contains("检视线与开发线是同一个模型"), "{doctor}");
}
