//! LIVE：真终端（testing.md §8）——伪终端里起真的 `conix`，键盘进、屏幕出（`live/tty.rs` 写了规矩）。
//! 端口驱动的 LIVE 测机制、断言细；这里是验收：人照着屏幕敲键，断言粗，落在屏幕与项目最后的样子上。
//!
//! - 起实例、`/trust`、^N 换标签、^X 后台运行（`conix list` 里还在跑）、`conix attach` 接回、^X 直接退出（停了）。
//! - 草稿上说一句话，看见回复与会话；`/rename` 改名。
//! - 权限询问：^G 走开、^R 找回、回到左边 Enter 批准，工具的输出出现在转录里。
//! - xreport：造好的历史上 `/flow:xreport 上周的提交 --days 7`，^R 跳到等确认的运行，看过 `REPORT.md`
//!   后 `/confirm 报告定稿`，归档里是批的那一份。
//!
//! 主模型是 Claude（`.env` 没有它的 key 就跳过）。

mod live;

use std::path::{Path, PathBuf};
use std::time::Duration;

use live::tty::{self, Step, Tty};
use live::{QUICK, TURN};

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

#[test]
#[ignore = "live"]
fn the_terminal_starts_an_instance_detaches_from_it_and_stops_it_on_quit() {
    let lv = claude_or_skip!();
    let mut t = Tty::new(&lv);
    let s = t.screen();
    assert!(s.contains("[CONIX] lv@"), "顶栏是这个实例");
    assert!(s.contains("新会话（输入即开始）"), "启动是草稿");

    t.line("/trust");
    t.wait_text("信任的回复", QUICK, "已信任这个项目");

    // ^N 轮到流程标签：项目里没装流程包；再两下回到会话。
    t.keys("^N");
    t.wait_text("流程标签", QUICK, "还没有流程");
    t.keys("^N ^N");
    t.wait_text("回到会话标签", QUICK, "新建会话");

    // ^X → 后台运行：界面退出，实例还在跑；接回来还是它。
    t.detach();
    assert_eq!(tty::listed_state(&lv).as_deref(), Some("Running"));
    let t = Tty::attach(&lv);
    assert!(t.contains("[CONIX] lv@"));
    // ^X → 直接退出：实例停了。
    t.quit();
    assert_eq!(tty::listed_state(&lv).as_deref(), Some("Stopped"));
}

#[test]
#[ignore = "live"]
fn a_line_typed_in_the_draft_gets_its_reply_and_rename_renames_the_session() {
    let lv = claude_or_skip!();
    let mut t = Tty::new(&lv);
    let s = t.say("Reply with exactly one word: pong", TURN);
    assert!(s.to_lowercase().contains("⏺ pong"), "回复在转录里");
    assert!(s.contains("#1"), "会话列表里有它");

    t.line("/rename 乒乓");
    t.wait("改名回来", QUICK, |s| {
        s.lines().nth(1).is_some_and(|l| l.contains("会话 › 乒乓"))
    });
    t.quit();
}

#[test]
#[ignore = "live"]
fn a_permission_prompt_is_found_with_ctrl_r_and_approved_with_enter() {
    let lv = claude_or_skip!();
    let mut t = Tty::new(&lv);
    // 带算术展开：纯 echo 是只读命令，引擎不问人。
    t.line(
        "Use the Bash tool to run exactly this command: echo live-tty-$((40+2)) . Then tell me what it printed.",
    );
    t.wait("等人批准", TURN, |s| s.contains(tty::ASK_ACTIVE));
    assert_eq!(t.chat_state(), Some(tty::WAITING));

    // 走开到右栏，提问框交出键盘；^R 找回，^G 转回左边，提问框又拿到键盘。
    t.keys("^G");
    t.wait("提问框交出键盘", QUICK, |s| {
        !s.contains(tty::ASK_ACTIVE)
    });
    t.keys("^R");
    t.wait_text("跳到待处理", QUICK, "待处理 1/1");
    t.keys("^G ^G");
    t.wait("回到提问框", QUICK, |s| s.contains(tty::ASK_ACTIVE));
    t.keys("Enter");
    let s = t.wait_idle("批准之后跑完", TURN, false);
    assert!(s.contains("live-tty-42"), "命令真跑了，输出在转录里");
    t.quit();
}

fn git(dir: &str, args: &[&str], env: &[(&str, &str)]) {
    let o = std::process::Command::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .envs(env.iter().copied())
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// 提交成 `name <email>`，作者时间与提交时间都是 `days_ago` 天前的上午十点（统计按提交时间筛）。
fn commit(dir: &str, name: &str, email: &str, days_ago: i64, msg: &str) {
    let when = (chrono::Local::now() - chrono::Duration::days(days_ago))
        .format("%Y-%m-%dT10:00:00%:z")
        .to_string();
    git(dir, &["add", "-A"], &[]);
    git(
        dir,
        &["commit", "-q", "-m", msg],
        &[
            ("GIT_AUTHOR_NAME", name),
            ("GIT_AUTHOR_EMAIL", email),
            ("GIT_AUTHOR_DATE", &when),
            ("GIT_COMMITTER_NAME", name),
            ("GIT_COMMITTER_EMAIL", email),
            ("GIT_COMMITTER_DATE", &when),
        ],
    );
}

fn write(dir: &str, rel: &str, text: &str) {
    let p = Path::new(dir).join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// 这一周里的三个提交：Alice Chen 两个（一个用旧名字 achen，`.mailmap` 并过来）、Bob Li 一个；示例项目
/// 的 `init` 是今天的（作者 live），也在范围里。
fn last_week(dir: &str) {
    write(
        dir,
        ".mailmap",
        "Alice Chen <alice@example.com> achen <alice@old.example>\n",
    );
    write(dir, "src/report.py", "def report():\n    return []\n");
    commit(
        dir,
        "Alice Chen",
        "alice@example.com",
        5,
        "加报告的骨架与 mailmap",
    );
    write(
        dir,
        "src/report.py",
        "def report(rows):\n    return list(rows)\n",
    );
    commit(dir, "achen", "alice@old.example", 4, "报告收行");
    write(
        dir,
        "docs/guide.md",
        "# 指南\n\n跑 report。\n结果是列表。\n",
    );
    commit(dir, "Bob Li", "bob@example.com", 2, "写使用指南");
}

fn find_archive(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.join("record.json").is_file())
}

/// 一次运行的上限：只有分析站用模型。
const RUN: Duration = Duration::from_secs(900);

#[test]
#[ignore = "live"]
fn xreport_is_started_confirmed_and_archived_from_the_keyboard() {
    let lv = claude_or_skip!();
    let to = lv.project_path();
    last_week(&to);
    // 人在 shell 里把包装进项目（包的管理只在命令行）。
    lv.conix_flow_ok(&["install", "xreport", "--yes", "--jobs", "no"]);

    let mut t = Tty::new(&lv);
    // 命令行先写了项目的 `.conix/`，实例第一次起来认不出 `state/`、挪走了，接入时弹「离开期间」说这件事。
    t.close_away();
    t.line("/trust");
    t.wait_text("信任的回复", QUICK, "已信任这个项目");

    // `--days 7` 打进来是字，实例照 xreport 的输入 schema 读成整数。
    t.line("/flow:xreport 上周的提交 --days 7");
    t.wait_text("启动", QUICK, "已启动「上周的提交」（");
    let run = t
        .between("已启动「上周的提交」（", "）")
        .expect("提示里有运行的 id");

    // 统计、分析都是自己跑的；定稿站等人确认时底栏的待处理变成 1，^R 跳到那次运行。
    t.drive("定稿等人确认", RUN, |t, _| {
        if t.attention().is_some_and(|n| n > 0) {
            Step::Done(())
        } else {
            Step::Wait
        }
    });
    t.keys("^R");
    t.wait("运行在等确认", QUICK, |s| {
        s.lines()
            .any(|l| l.contains("上周的提交") && l.contains(tty::WAITS_CONFIRM))
    });

    // 人先看一眼报告：数是 git 的，名字并过了，五节都在。
    let out = Path::new(&to).join("conix/runs").join(&run);
    let report = std::fs::read_to_string(out.join("REPORT.md"))
        .unwrap_or_else(|e| t.fail(&format!("{}：{e}", out.join("REPORT.md").display())));
    eprintln!("---- REPORT.md\n{report}");
    for want in [
        "Alice Chen",
        "Bob Li",
        "## 概览",
        "## 贡献者",
        "## 时间分布",
        "## 热点",
        "## 观察",
    ] {
        assert!(report.contains(want), "{want}\n{report}");
    }
    assert!(!report.contains("achen"), "旧名字并掉了：{report}");
    assert!(!report.contains("example.com"), "没有邮箱：{report}");

    // 回到输入框（右上 → 右下 → 输入框），流程标签里选中的就是这次运行：`/confirm` 不必写运行与站。
    t.keys("^G ^G");
    t.line("/confirm 报告定稿");
    t.wait_text("确认的回复", QUICK, &format!("已确认 {run} 的站 publish"));
    let archive_root = Path::new(&to).join("conix/xreport/archive");
    let archived = t.drive("归档", RUN, |_, s| match find_archive(&archive_root) {
        Some(a) if s.contains("已结束 (1)") => Step::Done(a),
        _ => Step::Wait,
    });
    for f in ["REPORT.md", "analysis.md", "stats.json", "record.json"] {
        assert!(archived.join(f).is_file(), "{}/{f}", archived.display());
    }
    assert_eq!(
        std::fs::read_to_string(archived.join("REPORT.md")).unwrap(),
        report,
        "归档的就是看过、批了的那一份"
    );
    t.quit();
}
