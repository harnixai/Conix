//! LIVE：真终端里的 xforge 验收（testing.md §8.1）——一个样例项目（`examples/xforge` 的 textkit），人只用键盘把它改三次。
//! 端口驱动的 `live_xforge.rs` 测机制；这里是验收：动作只走键盘，断言落在项目最后的样子上。
//!
//! 1. 人在 shell 里写好治理文件的三条线（开发线 Claude，测试线、检视线 DeepSeek）、`conix flow install xforge`，起 `conix`、`/trust`。
//! 2. 对话里说"用 xforge 加 truncate"，主 Agent 判风险走 quick；跑到验收，`/confirm`；人在 shell 里提交这次的改动。
//! 3. `/flow:xforge:solid` 加 `wrap`：两路，`/confirm` 规格侧、两路对上、验收，`/worktree merge`。
//! 4. 人改治理文件打开全链条检视，`/flow:xforge:major` 把 `word_count` 改名为 `count_words`（破坏性）：
//!    答澄清的问题（`/flow:xforge:xledger --run <运行> set questions … answered …`，再 `/flow:xforge:clarify <运行>`），
//!    批准破坏性改动，等检视结清，验收，`/worktree merge`。
//!
//! 收尾：项目的验证过；规格基线多出三条需求，接口登记里有 `count_words`、没有 `word_count`；三份归档，`conix flow verify --all`
//! 干净，`conix flow doctor` 没有 error；git 历史里三次变更各一处；屏幕上三次运行都结束了。
//!
//! 人在 Conix 之外做的（写治理文件、提交）是普通的文件读写与命令；做决定时人会读项目里的文件（运行的台账、记录），这里也读。
//! 每条路径另有一个短用例，调试时不用跑全套。三次连着约 1.5–2 小时。主模型是 Claude（`.env` 没有它的 key 就跳过）。

mod live;

use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

use live::tty::{self, ASK_ACTIVE, Step, Tty};
use live::{Live, QUICK, TURN};
use serde_json::Value;

/// 一次变更的上限。
const CHANGE: Duration = Duration::from_secs(2 * 3600);

macro_rules! claude_or_skip {
    () => {
        match tty::on_claude(live::Opts::default()) {
            Some(lv) => lv,
            None => {
                eprintln!("skipped: .env has no CLAUDE_API_KEY / CLAUDE_MODEL");
                return;
            }
        }
    };
}

fn example() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/xforge"
    ))
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

fn git_commit(dir: &str, msg: &str) {
    sh(
        dir,
        &format!(
            "git add -A && git -c user.name=person -c user.email=person@conix commit -qm '{msg}'"
        ),
    );
}

/// 样例项目：textkit，不带包（人用 `conix flow install` 装内置的那份）；治理文件里写好三条线的模型。
fn sample(lv: &Live) -> String {
    let to = lv.project_path();
    sh(
        &to,
        &format!("cp -R {}/. . && rm -rf .conix/flows", example().display()),
    );
    let (cl, ds) = (live::claude().unwrap(), live::deepseek());
    let gov = Path::new(&to).join(".conix/governance/xforge.yaml");
    let text = std::fs::read_to_string(&gov).unwrap();
    let (head, rest) = text.split_once("lines:\n").expect("演示的治理文件有 lines");
    let (_, tail) = rest.split_once("\n\n").expect("lines 之后有空行");
    let lines = format!(
        "  build: {{model: {}, provider: claude}}\n  tests: {{model: {}, provider: deepseek}}\n  review: {{model: {}}}",
        cl.model, ds.model, ds.model
    );
    std::fs::write(&gov, format!("{head}lines:\n{lines}\n\n{tail}")).unwrap();
    git_commit(&to, "textkit");
    lv.conix_flow_ok(&["install", "xforge", "--yes", "--jobs", "no"]);
    git_commit(&to, "装上 xforge");
    to
}

/// 起 `conix`，关掉「离开期间」（命令行先写了 `.conix/`），`/trust`。
fn open(lv: &Live) -> Tty<'_> {
    let mut t = Tty::new(lv);
    t.close_away();
    t.line("/trust");
    t.wait_text("信任的回复", QUICK, "已信任这个项目");
    t
}

fn runs(lv: &Live) -> Vec<(String, Value)> {
    let dir = lv.state().flow_runs();
    std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let id = name.strip_suffix(".json")?.to_string();
            let v: Value = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            Some((id, v))
        })
        .collect()
}

fn record(lv: &Live, run: &str) -> Value {
    lv.try_state(&format!("flow-runs/{run}.json"))
        .unwrap_or(Value::Null)
}

/// 刚起的运行：屏幕上出了「已启动「…」」，项目里有了一条输入 `title` 是它的运行记录（人在流程标签里也看得到它）。
fn started(t: &mut Tty, lv: &Live, title: &str) -> String {
    t.wait_text("运行起来了", QUICK, "已启动「");
    t.drive("运行的记录", QUICK, |_, _| {
        match runs(lv)
            .into_iter()
            .find(|(_, v)| v["inputs"]["title"] == title)
        {
            Some((id, _)) => Step::Done(id),
            None => Step::Wait,
        }
    })
}

/// 一站在等人确认：在做（working），有一条在等（wait）的确认。
fn waits_confirm(st: &Value) -> bool {
    st["status"] == "working"
        && st["checks"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|c| c["kind"] == "confirm" && c["status"] == "wait")
}

/// 敲一条斜杠命令，同一件事（`key`：运行、站、轮次）只敲一次；之后几秒不看，等实例把它做完。
fn act(
    t: &mut Tty,
    done: &mut BTreeSet<String>,
    next_look: &mut Instant,
    key: String,
    cmd: String,
) {
    if done.insert(key) {
        t.command(&cmd);
        *next_look = Instant::now() + Duration::from_secs(5);
    }
}

/// 人打回一处的次数，至多 `max`：免得模型绕圈时一直烧钱。
fn again(reworks: &mut std::collections::BTreeMap<String, u32>, key: &str, max: u32) {
    let n = reworks.entry(key.to_string()).or_default();
    *n += 1;
    assert!(*n <= max, "{key} 人打回了 {max} 次还没过");
}

/// 一次运行连同它的子运行，演人把它跑完：批准冒出来的权限询问，确认等人的站，答澄清的问题，没达标时照建议打回
/// （都有上限）。每个动作都是在输入框里敲的斜杠命令。回结束时的记录。
fn finish(t: &mut Tty, lv: &Live, run: &str, limit: Duration) -> Value {
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut reworks: std::collections::BTreeMap<String, u32> = Default::default();
    let mut next_look = Instant::now();
    t.drive(&format!("{run} 跑完"), limit, |t, s| {
        if s.contains(ASK_ACTIVE) {
            t.keys("Enter");
            return Step::Wait;
        }
        if Instant::now() < next_look {
            return Step::Wait;
        }
        next_look = Instant::now() + Duration::from_secs(3);
        let rec = record(lv, run);
        if rec["status"] == "finished" {
            return Step::Done(rec);
        }
        assert_ne!(rec["status"], "cancelled", "{run} 被取消了");
        // 子运行：无人值守，自动打回用完了就照写它的那一站再打回。
        for task in rec["stations"]["fork"]["tasks"].as_array().into_iter().flatten() {
            let Some(child) = task["child"].as_str() else { continue };
            let c = record(lv, child);
            for (st, r) in c["stations"].as_object().into_iter().flatten() {
                if r["status"] == "unmet" {
                    let key = format!("{child} {st} {}", r["round"]);
                    if !done.contains(&key) {
                        again(&mut reworks, &format!("{child} {st}"), 2);
                        let to = if matches!(st.as_str(), "apply" | "review-code" | "review-tests") { "apply" } else { "design" };
                        act(t, &mut done, &mut next_look, key, format!("/rework {child} {to} 没达标的照台账与门禁的说明改，规格侧不动"));
                    }
                }
            }
        }
        for (st, r) in rec["stations"].as_object().into_iter().flatten() {
            let key = format!("{run} {st} {}", r["round"]);
            if waits_confirm(r) {
                let why = match st.as_str() {
                    "propose" => "破坏性改动与影响面看过了，批准",
                    "clarify" => "范围定了",
                    "seal" => "规格侧定了",
                    "join" => "两路对上了",
                    _ => "验收通过",
                };
                act(t, &mut done, &mut next_look, key, format!("/confirm {run} {st} {why}"));
            } else if r["status"] == "unmet" && !done.contains(&key) {
                match st.as_str() {
                    // 澄清的问题只有人答：人读台账里的问题，照默认答，再让这一站重判。
                    "clarify" => {
                        let out = rec["output_dir"].as_str().unwrap_or_default();
                        let qs = std::fs::read_to_string(Path::new(out).join("ledgers/questions.yaml")).unwrap_or_default();
                        let doc: serde_yaml::Value = serde_yaml::from_str(&qs).unwrap_or_default();
                        for q in doc["entries"].as_sequence().into_iter().flatten().filter(|q| q["conclusion"].as_str() == Some("open")) {
                            let id = q["id"].as_str().unwrap_or_default();
                            let text = q["text"].as_str().unwrap_or_default();
                            let answer = text.split_once("默认：").map(|(_, d)| format!("照默认：{}", d.trim())).unwrap_or_else(|| "照你建议的做，范围不扩大".into());
                            let answer = answer.replace('"', "'");
                            t.command(&format!("/flow:xforge:xledger --run {run} set questions {id} answered \"{answer}\""));
                            t.wait_text("答了一条", QUICK, &format!("{id} → answered"));
                        }
                        act(t, &mut done, &mut next_look, key, format!("/flow:xforge:clarify {run} 问题答完了"));
                    }
                    // 合起来没对上：照会话的建议只重做那一路（判不出就两路）。
                    "join" => {
                        again(&mut reworks, "join", 2);
                        let picked: Vec<String> = r["tasks"]
                            .as_array()
                            .and_then(|t| t.last())
                            .and_then(|t| t["output"]["rework"]["tasks"].as_array())
                            .map(|t| t.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                            .unwrap_or_default();
                        let to = if picked.is_empty() { "fork".to_string() } else { format!("fork/{}", picked.join(",")) };
                        act(t, &mut done, &mut next_look, key, format!("/rework {run} {to} 黑盒测试没过：照 findings 台账按规格修，不照对面凑"));
                    }
                    "lines" => panic!("三条线没核对过：{}", r["checks"]),
                    other => {
                        again(&mut reworks, other, 2);
                        let to = match other {
                            "review-spec" => "propose",
                            "review-verify" => "verify",
                            s => s,
                        };
                        act(t, &mut done, &mut next_look, key, format!("/rework {run} {to} 还没达标：照台账与门禁的说明改"));
                    }
                }
            }
        }
        Step::Wait
    })
}

/// 运行在自己的 worktree 里的，人看过之后合进项目：`/worktree <运行> merge`。
fn merge(t: &mut Tty, lv: &Live, run: &str) {
    t.command(&format!("/worktree {run} merge"));
    t.drive("合进项目", QUICK, |_, _| {
        if record(lv, run)["worktree"]["outcome"]["kind"] == "merged" {
            Step::Done(())
        } else {
            Step::Wait
        }
    });
}

fn shows_finished(t: &mut Tty, lv: &Live, changes: usize) {
    // 流程标签里结束的运行计数（`已结束 (n)`）只数人起的那几次：两路的子运行收在父运行底下。
    let ended: Vec<String> = runs(lv)
        .into_iter()
        .filter(|(_, v)| v["status"] == "finished")
        .map(|(id, _)| id)
        .collect();
    let top = runs(lv)
        .into_iter()
        .filter(|(_, v)| v["status"] == "finished" && v["parent"].is_null())
        .count();
    assert_eq!(top, changes, "结束的变更：{ended:?}");
    t.keys("^N");
    t.wait_text("流程标签", QUICK, "已结束");
    t.wait_text("结束的计数", QUICK, &format!("已结束 ({changes})"));
    t.keys("^N ^N Esc");
}

/// 第一次：在对话里说，主 Agent 判风险、启动；只有主人验收。
fn change_one(t: &mut Tty, lv: &Live) -> String {
    let before: BTreeSet<String> = runs(lv).into_iter().map(|(id, _)| id).collect();
    t.say(
        "用 xforge 给 textkit 加 truncate(text, n)：文字超过 n 个字符时截成前 n-1 个字符再加一个省略号 …，不超过时原样返回。\
         这是低风险的小改动（只加一个函数），标题 truncate。",
        TURN,
    );
    let run = t.drive(
        "主 Agent 起了运行",
        Duration::from_secs(300),
        |t, s| {
            if s.contains(ASK_ACTIVE) {
                t.keys("Enter");
                return Step::Wait;
            }
            match runs(lv).into_iter().find(|(id, v)| {
                !before.contains(id) && v["flow"].as_str().is_some_and(|f| f.starts_with("xforge/"))
            }) {
                Some((id, _)) => Step::Done(id),
                None => Step::Wait,
            }
        },
    );
    let rec = finish(t, lv, &run, CHANGE);
    assert_eq!(
        rec["flow"], "xforge/quick",
        "主 Agent 判的是低风险：{}",
        rec["routed"]
    );
    run
}

/// 第二次：点名 solid，两路。
fn change_two(t: &mut Tty, lv: &Live) -> String {
    t.command(
        "/flow:xforge:solid 给 textkit 加 wrap(text, width)：按 width 个字符折行，返回行的列表；空串返回空列表；width 小于 1 时抛 ValueError --risk medium --title wrap",
    );
    let run = started(t, lv, "wrap");
    finish(t, lv, &run, CHANGE);
    merge(t, lv, &run);
    run
}

/// 第三次：人先打开全链条检视（改治理文件、提交），再点名 major 做一个破坏性的改名。
fn change_three(t: &mut Tty, lv: &Live, to: &str) -> String {
    let gov = Path::new(to).join(".conix/governance/xforge.yaml");
    let text = std::fs::read_to_string(&gov).unwrap();
    let full = text.replace(
        "major: {spec: true, design: true, tests-design: true, code: false, tests: false, verify: false}",
        "major: {spec: true, design: true, tests-design: true, code: true, tests: true, verify: true}",
    );
    assert_ne!(full, text, "演示的治理文件改了，这里跟着改");
    std::fs::write(&gov, full).unwrap();
    git_commit(to, "全链条检视");
    t.command(
        "/flow:xforge:major 把 textkit 的 word_count 改名为 count_words：旧名直接删掉，不留兼容（调用方都在这个仓库里） --risk high --impact interface --title count-words",
    );
    let run = started(t, lv, "count-words");
    let rec = finish(t, lv, &run, CHANGE);
    for st in ["review-spec", "review-verify"] {
        assert_eq!(rec["stations"][st]["status"], "passed", "{st}：全链条开着");
    }
    merge(t, lv, &run);
    run
}

#[test]
#[ignore = "live"]
fn a_sample_project_is_changed_three_times_by_keyboard_through_quick_solid_and_major() {
    let lv = claude_or_skip!();
    let to = sample(&lv);
    let specs_before = std::fs::read_to_string(Path::new(&to).join("xforge/specs/textkit.md"))
        .unwrap()
        .matches("\n### ")
        .count();
    let mut t = open(&lv);
    let one = change_one(&mut t, &lv);
    // quick 改在项目的工作区里，人看过、提交了再起下一个：worktree 的运行合回来时 git 不往没提交的改动上合。
    git_commit(&to, "加 truncate");
    let two = change_two(&mut t, &lv);
    let three = change_three(&mut t, &lv, &to);
    shows_finished(&mut t, &lv, 3);
    t.quit();

    // 项目的验证过；三次的改动都在。
    eprintln!(
        "---- verify\n{}",
        sh(&to, "sh .conix/governance/verify.sh 2>&1")
    );
    let src = std::fs::read_to_string(Path::new(&to).join("src/textkit/__init__.py")).unwrap();
    assert!(
        src.contains("def truncate") && src.contains("def wrap") && src.contains("def count_words"),
        "{src}"
    );
    assert!(!src.contains("def word_count"), "旧名删了：{src}");
    // 规格基线多出三条需求（改名可能改已有的一条，也可能加一条：至少两条新的，一共动了三处）；接口登记换了名。
    let spec = std::fs::read_to_string(Path::new(&to).join("xforge/specs/textkit.md")).unwrap();
    assert!(spec.matches("\n### ").count() >= specs_before + 2, "{spec}");
    let api =
        std::fs::read_to_string(Path::new(&to).join("xforge/interfaces/textkit.yaml")).unwrap();
    assert!(
        api.contains("count_words")
            && api.contains("fn:textkit.truncate")
            && api.contains("fn:textkit.wrap"),
        "{api}"
    );
    // 三份归档，核对干净；自检没有 error。
    let archives: Vec<_> = std::fs::read_dir(Path::new(&to).join("xforge/archive"))
        .unwrap()
        .flatten()
        .filter(|e| e.path().join("record.json").is_file())
        .collect();
    assert_eq!(archives.len(), 3, "{archives:?}");
    let verified = lv.conix_flow(&["verify", "--all"]);
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stdout)
    );
    let doctor = lv.conix_flow(&["doctor"]);
    assert!(
        doctor.status.success(),
        "{}",
        String::from_utf8_lossy(&doctor.stdout)
    );
    // git 历史里三次变更各一处：quick 在项目的分支上（人提交），solid、major 是合进来的分支。
    let log = sh(&to, "git log --oneline --all");
    eprintln!("---- git log\n{log}");
    for run in [&two, &three] {
        let rec = record(&lv, run);
        let branch = rec["worktree"]["branch"].as_str().unwrap();
        assert!(
            sh(&to, &format!("git log --oneline --grep '{run}'"))
                .lines()
                .count()
                >= 1
                || log.contains(branch),
            "{run} 的提交在历史里"
        );
    }
    assert!(
        record(&lv, &one)["worktree"].is_null(),
        "quick 在项目里改的"
    );
}

#[test]
#[ignore = "live"]
fn quick_from_the_conversation_by_keyboard() {
    let lv = claude_or_skip!();
    sample(&lv);
    let mut t = open(&lv);
    change_one(&mut t, &lv);
    shows_finished(&mut t, &lv, 1);
    t.quit();
}

#[test]
#[ignore = "live"]
fn solid_by_keyboard() {
    let lv = claude_or_skip!();
    sample(&lv);
    let mut t = open(&lv);
    change_two(&mut t, &lv);
    shows_finished(&mut t, &lv, 1);
    t.quit();
}

#[test]
#[ignore = "live"]
fn major_with_the_whole_chain_reviewed_by_keyboard() {
    let lv = claude_or_skip!();
    let to = sample(&lv);
    let mut t = open(&lv);
    change_three(&mut t, &lv, &to);
    shows_finished(&mut t, &lv, 1);
    t.quit();
}
