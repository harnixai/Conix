//! 场景 m01–m08、m16：对话区（需求 C-01 至 C-21，交互 §4，照 Claude Code）。

mod common;

use app::core::ShellEffect;
use common::Harness;
use port::{Answer, Command, Event, Prompt, PromptKind};
use tui::Size;

const SIZE: Size = Size::new(160, 45);
const SESSION: &str = "昨天的调研";

async fn chat(scenario: &str) -> Harness {
    let mut h = Harness::new(scenario, SIZE).await;
    h.open_session(SESSION).await;
    h
}

async fn ask(h: &mut Harness, text: &str) {
    h.type_str(text).await;
    h.keys("Enter").await;
}

fn submitted(h: &Harness) -> Vec<String> {
    h.calls("dev")
        .into_iter()
        .filter_map(|c| match c {
            Command::Submit { text, .. } => Some(text),
            _ => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn m01_streams_with_thinking_status_and_markdown() {
    let mut h = chat("m01-chat-basic").await;
    assert!(
        h.transcript()
            .contains(&format!("Conix {}", model::short_version())),
        "欢迎块在最上面"
    );
    ask(&mut h, "介绍一下").await;
    h.advance(150).await;
    let text = h.text();
    assert!(text.contains("思考中") || text.contains("回答中"), "{text}");
    assert!(text.contains("esc 中断"), "运行中输入框下显示 esc 中断");
    h.advance(3_000).await;
    let t = h.transcript();
    assert!(t.contains("> 介绍一下"), "{t}");
    assert!(t.contains("✻ 思考了"), "思考折成一行：{t}");
    assert!(t.contains("⏺ "), "{t}");
    assert!(t.contains("• "), "列表：{t}");
    let text = h.text();
    assert!(!text.contains("esc 中断"));
    assert!(text.contains("? 快捷键"));
    h.snapshot("m01-chat-basic");
}

#[tokio::test(start_paused = true)]
async fn m02_hard_text_never_overflows() {
    let mut h = chat("m02-text-hard").await;
    ask(&mut h, "来点难排的").await;
    h.advance(5_000).await;
    let width = h.core.state.chat_regions().chat.unwrap().transcript.w as usize;
    let conv = h.core.state.conv().unwrap();
    for l in conv.lines() {
        assert!(l.width() <= width, "超宽：{:?}", l.text());
        assert!(!l.text().contains(['\n', '\r']), "含换行：{:?}", l.text());
    }
    assert!(h.transcript().contains("🚀"));
}

#[tokio::test(start_paused = true)]
async fn m03_tools_render_like_claude_code_and_ctrl_o_expands() {
    let mut h = chat("m03-tools").await;
    ask(&mut h, "改一下校验").await;
    h.advance(5_000).await;
    let t = h.transcript();
    for want in [
        "⏺ Read(src/auth/mod.rs)",
        "读取了 42 行",
        "⏺ Grep(fn verify)",
        "更新了 src/auth/mod.rs，+",
        "⏺ Bash(cargo test auth)",
        "(ctrl+o 展开)",
        "Error:",
    ] {
        assert!(t.contains(want), "缺 {want}\n{t}");
    }
    let compact = h.core.state.conv().unwrap().total_lines();
    h.keys("Ctrl+O").await;
    assert_eq!(h.core.state.ui.focus, model::Focus::Transcript, "看转录");
    let detail = h.core.state.conv().unwrap().total_lines();
    assert!(
        detail > compact + 30,
        "详细模式展开全部输出：{compact} → {detail}"
    );
    assert!(!h.transcript().contains("读取了 42 行"));
    h.keys("Ctrl+O").await;
    assert_eq!(h.core.state.ui.focus, model::Focus::Composer, "回输入框");
    assert_eq!(
        h.core.state.conv().unwrap().total_lines(),
        compact,
        "收回精简"
    );
    h.snapshot("m03-tools");
}

#[tokio::test(start_paused = true)]
async fn m03_select_block_and_toggle_with_enter() {
    let mut h = chat("m03-tools").await;
    ask(&mut h, "改一下校验").await;
    h.advance(5_000).await;
    let before = h.core.state.conv().unwrap().total_lines();
    h.keys("Ctrl+O o [ [ Enter").await;
    let after = h.core.state.conv().unwrap().total_lines();
    assert!(after > before, "展开了一个块");
    h.keys("Enter").await;
    assert_eq!(h.core.state.conv().unwrap().total_lines(), before);
}

#[tokio::test(start_paused = true)]
async fn m04_permission_question_and_withdrawn() {
    let mut h = chat("m04-prompts").await;
    ask(&mut h, "跑测试").await;
    h.advance(600).await;
    let text = h.text();
    assert!(text.contains("Bash 命令"), "{text}");
    assert!(text.contains("(1/3)"));
    assert!(text.contains("是，且不再询问：cargo test --workspace"));
    assert!(text.contains("来自 reviewer（子 Agent）"));
    h.snapshot("m04-ask-permission");
    h.keys("1").await;
    h.advance(300).await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::Answer { prompt, answer: Answer::Permit, .. } if prompt.as_str() == "p1"
    )));
    assert!(h.text().contains("用哪个数据库？"));
    h.keys("2").await;
    h.advance(300).await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::Answer {
            answer: Answer::Choice { index: 1 },
            ..
        }
    )));
    h.advance(9_000).await;
    assert!(
        h.core.state.conv().unwrap().prompts.is_empty(),
        "p3 超时撤回"
    );
}

#[tokio::test(start_paused = true)]
async fn m04_a_prompt_without_always_offers_yes_and_no_and_names_the_plugin_command() {
    let mut h = chat("default").await;
    let session = h.core.state.ui.chat.clone().unwrap();
    h.core
        .handle(model::Msg::Event(Box::new(Event::PromptPending {
            session,
            prompt: Prompt {
                id: "px".into(),
                kind: PromptKind::Permission {
                    tool: "Write".into(),
                    summary: "xforge/changes/a.md".into(),
                    paths: vec![],
                },
                from: None,
                asked_ms: 0,
                permit_always: false,
                via: Some("/xforge spec new a".into()),
            },
        })));
    h.settle().await;
    let text = h.text();
    assert!(!text.contains("不再询问"), "{text}");
    assert!(text.contains("经插件命令 /xforge spec new a"), "{text}");
    h.keys("2").await;
    h.advance(300).await;
    assert!(
        h.calls("dev").iter().any(|c| matches!(
            c,
            Command::Answer {
                answer: Answer::Deny { .. },
                ..
            }
        )),
        "只有两项时第 2 项是否"
    );
}

#[tokio::test(start_paused = true)]
async fn m04_ask_does_not_steal_focus_while_typing() {
    let mut h = chat("default").await;
    h.type_str("正在打").await;
    let session = h.core.state.ui.chat.clone().unwrap();
    h.core
        .handle(model::Msg::Event(Box::new(Event::PromptPending {
            session,
            prompt: Prompt {
                id: "px".into(),
                kind: PromptKind::Permission {
                    tool: "Bash".into(),
                    summary: "ls".into(),
                    paths: vec![],
                },
                from: None,
                asked_ms: 0,
                permit_always: true,
                via: None,
            },
        })));
    h.type_str("y").await;
    assert_eq!(
        h.core.state.ui.composer.text, "正在打y",
        "y 进了输入框，没有变成回答"
    );
    assert!(
        !h.calls("dev")
            .iter()
            .any(|c| matches!(c, Command::Answer { .. }))
    );
    h.keys("Ctrl+R Esc").await;
    assert!(
        h.calls("dev").iter().any(|c| matches!(
            c,
            Command::Answer {
                answer: Answer::Deny { .. },
                ..
            }
        )),
        "Esc 在提问框里就是拒绝"
    );
}

#[tokio::test(start_paused = true)]
async fn m05_esc_interrupts_a_running_turn() {
    let mut h = chat("m05-interrupt").await;
    ask(&mut h, "讲个长的").await;
    h.advance(800).await;
    h.keys("Esc").await;
    h.advance(500).await;
    assert!(
        h.calls("dev")
            .iter()
            .any(|c| matches!(c, Command::Interrupt { .. }))
    );
    assert!(h.transcript().contains("已中断"));
    assert!(!h.core.state.conv().unwrap().running);
    ask(&mut h, "再来").await;
    h.advance(20).await;
    assert!(h.core.state.conv().unwrap().running, "中断后马上能再提交");
}

#[tokio::test(start_paused = true)]
async fn m06_step_restart_discards_partial_text() {
    let mut h = chat("m06-step-restart").await;
    ask(&mut h, "试试").await;
    h.advance(3_000).await;
    let t = h.transcript();
    assert!(t.contains("重新生成的完整回答"));
    assert!(!t.contains("第一次尝试的半句话"), "{t}");
    assert!(t.contains("正在重试"));
}

#[tokio::test(start_paused = true)]
async fn m07_steer_delivered_and_returned() {
    let mut h = chat("m07-steer").await;
    ask(&mut h, "开始").await;
    h.advance(300).await;
    ask(&mut h, "补充一句").await;
    h.settle().await;
    assert!(h.transcript().contains("⧗ 待送达"), "{}", h.transcript());
    h.advance(400).await;
    assert!(h.transcript().contains("↳ 已插话"));
    ask(&mut h, "太晚了").await;
    h.advance(200).await;
    assert_eq!(
        h.core.state.ui.composer.text, "太晚了",
        "被退回的插话放回输入框"
    );
    assert!(!h.transcript().contains("> 太晚了"));
}

#[tokio::test(start_paused = true)]
async fn m08_composer_picker_modes_and_keys() {
    let mut h = chat("default").await;
    h.type_str("/he").await;
    assert!(h.text().contains("/help"));
    h.keys("Tab").await;
    assert_eq!(h.core.state.ui.composer.text, "/help ");
    h.keys("Enter").await;
    assert!(h.core.state.ui.overlay.is_some(), "/help 打开帮助");
    h.keys("Esc").await;
    h.type_str("?").await;
    assert!(h.core.state.ui.overlay.is_some(), "空输入框按 ? 打开快捷键");
    h.keys("Esc").await;
    h.type_str("!ls").await;
    assert!(h.text().contains("! ls"), "命令模式");
    h.keys("Ctrl+U Backspace").await;
    assert!(!h.core.state.ui.composer.bash);
    h.keys("BackTab").await;
    assert!(h.text().contains("自动接受编辑"));
    h.keys("BackTab").await;
    assert!(h.text().contains("计划模式"));
    h.keys("BackTab").await;
    h.type_str("第一行").await;
    h.keys("Ctrl+J").await;
    h.type_str("第二行").await;
    assert_eq!(h.core.state.ui.composer.text, "第一行\n第二行");
    h.keys("Esc Esc").await;
    assert!(
        h.core.state.ui.composer.text.is_empty(),
        "连按两次 Esc 清空"
    );
    h.keys("Up").await;
    assert_eq!(
        h.core.state.ui.composer.text, "第一行\n第二行",
        "清空的存进了历史"
    );
    h.keys("Ctrl+U").await;
    h.core.handle(model::Msg::Paste(
        (1..=12)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n"),
    ));
    assert_eq!(h.core.state.ui.composer.text, "[粘贴了 12 行 #1]");
    h.keys("Enter").await;
    let sent = submitted(&h);
    assert!(sent.last().unwrap().contains("line 12"), "提交时展开");
}

#[tokio::test(start_paused = true)]
async fn m08_ctrl_c_and_ctrl_d_like_claude_code() {
    let mut h = chat("default").await;
    h.type_str("abc").await;
    h.keys("Left Left Ctrl+D").await;
    assert_eq!(h.core.state.ui.composer.text, "ac", "有字时删后一字");
    h.keys("Ctrl+C").await;
    assert!(h.core.state.ui.composer.text.is_empty(), "Ctrl+C 先清空");
    h.keys("Ctrl+D").await;
    assert!(
        !h.shell
            .iter()
            .any(|e| matches!(e, ShellEffect::Quit { .. }))
    );
    h.keys("Ctrl+D").await;
    assert!(matches!(
        h.core.state.ui.overlay,
        Some(model::Overlay::Exit { .. })
    ));
    h.keys("Enter").await;
    assert!(h.shell.contains(&ShellEffect::Quit { detach: true }));
}

#[tokio::test(start_paused = true)]
async fn first_message_without_a_session_creates_one() {
    let mut h = Harness::new("default", SIZE).await;
    assert!(h.core.state.ui.chat.is_none());
    ask(&mut h, "你好").await;
    h.advance(1_000).await;
    assert!(h.core.state.ui.chat.is_some());
    let t = h.transcript();
    assert!(t.contains("> 你好"), "{t}");
    assert!(t.contains("（mock）收到：你好"), "{t}");
}

#[tokio::test(start_paused = true)]
async fn m16_storm_relayouts_one_block_per_delta() {
    let mut h = chat("m16-storm").await;
    ask(&mut h, "来吧").await;
    let before = h.core.state.ui.relayouts;
    h.advance(3_000).await;
    let spent = h.core.state.ui.relayouts - before;
    assert!(h.transcript().contains("字字字"));
    assert!(spent <= 1_100, "1000 个增量重排了 {spent} 个块");
}

#[tokio::test(start_paused = true)]
async fn demo_history_renders_every_block_kind() {
    let mut h = Harness::new("demo", SIZE).await;
    h.open_session("Conix 展示").await;
    let t = h.transcript();
    for want in [
        "> ",
        "✻ 思考了",
        "┌─ ",
        "│ ",
        "─┼─",
        "☒ ",
        "▎ ",
        "◦ ",
        "⏺ Read(",
        "读取了",
        "⏺ Edit(",
        "更新了",
        "⏺ Write(",
        "新建了",
        "⏺ TodoWrite",
        "Error:",
        "上下文已压缩",
    ] {
        assert!(t.contains(want), "缺 {want}");
    }
    h.snapshot("demo-history");
    ask(&mut h, "继续").await;
    h.advance(5_000).await;
    assert!(
        h.text().contains("git push origin feature/oauth"),
        "停在审批上"
    );
    h.keys("1").await;
    h.advance(5_000).await;
    assert!(!h.core.state.conv().unwrap().running);
}

/// 转录里按 y：没选中块就复制最近一次回复的原文，交给终端剪贴板（需求 M-07）。
#[tokio::test(start_paused = true)]
async fn y_in_the_transcript_copies_the_last_reply() {
    let mut h = Harness::new("default", Size::new(160, 45)).await;
    h.open_session("重构认证模块").await;
    h.settle().await;
    let last = h
        .core
        .state
        .conv()
        .and_then(|c| {
            c.blocks.iter().rev().find_map(|b| match &b.kind {
                model::BlockKind::Assistant { text } => Some(text.clone()),
                _ => None,
            })
        })
        .expect("这个会话里有回复");
    h.core.state.ui.focus = model::Focus::Transcript;
    let fx = h.core.handle(model::Msg::Action(keys::Action::Copy));
    assert!(
        fx.iter()
            .any(|e| matches!(e, ShellEffect::Clipboard(t) if *t == last)),
        "{fx:?}"
    );
}

/// `/model` 不带参数：列出各 provider 的模型，选一个就换（带上它的 provider，需求 C-13）。
#[tokio::test(start_paused = true)]
async fn slash_model_opens_a_picker_and_switches() {
    let mut h = Harness::new("default", Size::new(160, 45)).await;
    h.open_session("重构认证模块").await;
    h.type_str("/model").await;
    h.keys("Enter").await;
    h.settle().await;
    assert!(
        matches!(h.core.state.ui.overlay, Some(model::Overlay::Models { ref models, .. }) if models.len() == 3),
        "{:?}",
        h.core.state.ui.overlay
    );
    assert!(h.text().contains("claude-opus-5-5"));
    h.keys("Down Enter").await;
    h.settle().await;
    assert!(h.core.state.ui.overlay.is_none());
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::SetModel { provider: Some(p), .. } if p == "anthropic"
    )));
}

/// 输入框里打 `@`：取一次项目文件，往下打就在本地筛，文件名里就有的排前面；Tab 换成路径（需求 C-21）。
#[tokio::test(start_paused = true)]
async fn at_completes_project_files() {
    let mut h = Harness::new("default", Size::new(160, 45)).await;
    h.open_session("重构认证模块").await;
    h.type_str("看看 @").await;
    h.settle().await;
    h.type_str("oau").await;
    h.settle().await;
    let picker = h.core.state.picker_candidates();
    assert_eq!(
        picker.first().map(|c| c.name.as_str()),
        Some("src/auth/oauth.rs"),
        "{picker:?}"
    );
    assert_eq!(
        h.calls("dev")
            .iter()
            .filter(|c| matches!(c, Command::ProjectFiles))
            .count(),
        1,
        "只取一次"
    );
    h.keys("Tab").await;
    assert_eq!(h.core.state.ui.composer.text, "看看 @src/auth/oauth.rs ");
    assert!(
        h.core.state.picker_candidates().is_empty(),
        "空格之后不再补全"
    );
}

/// 输入框为空时连按两次 Esc：列出能退回去的轮，选一轮退回到它之前，那句话放回输入框改了再发（C-21）。
#[tokio::test(start_paused = true)]
async fn double_esc_rewinds_to_before_a_turn_and_puts_it_back() {
    let mut h = Harness::new("default", Size::new(160, 45)).await;
    h.open_session("昨天的调研").await;
    h.core.state.ui.focus = model::Focus::Composer;
    h.keys("Esc Esc").await;
    h.settle().await;
    assert!(
        matches!(h.core.state.ui.overlay, Some(model::Overlay::Rewind { ref turns, selected, .. })
            if turns.len() == 2 && selected == 1),
        "{:?}",
        h.core.state.ui.overlay
    );
    h.keys("Enter").await;
    h.settle().await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::Rewind { to_turn: Some(t), scope: port::RewindScope::Both, .. } if t == "t1"
    )));
    assert_eq!(h.core.state.ui.composer.text, "把 session 存储挪到 Redis");
}
