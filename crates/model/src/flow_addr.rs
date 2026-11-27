//! 流程地址（design/flows.md §4.3）：启动与操作是同一套名字。
//!
//! 全局：`/flow:<包>`、`/flow:<包> <主题> [--k v] [--step]`、`/flow:<包>:<流程> <主题> …`、
//! `/flow:<包>:<站> [运行] [理由]`、`/flow:<包>:<命令> [--run r] <参数>`。
//! 流程上下文（左边是某个运行的会话）：`/<包>`、`/<包>:<站> [理由]`、`/<包>:<流程> [理由]`、`/<包>:<命令> <参数>`，
//! 只有 `<包>` 正是这个运行的包时才由这里接，别的照旧交给 Agix。
//!
//! 都是本地路由（同 `/confirm`）：发端口命令，结果当本地输出显示，不进模型。名字属于流程、站还是命令
//! 由实例给的目录（`ListBundles`）定；站操作做什么由实例按运行状态定，这里不判断。
//! 不在包里的流程，包名就是流程名。

use port::{
    BundleInfo, Command, Pace, ProposalStatus, ProposalView, RunId, RunOp, RunStatus, RunSummary,
    RunView, SessionId, SlashCommand, StationId, StationStatus,
};

use crate::state::{Overlay, State};
use crate::update::{Effect, Pending};

/// 补全候选的种类标签放在 `SlashCommand::source`，界面按它找显示的字。
pub(crate) const SRC_BUNDLE: &str = "flow-bundle";
pub(crate) const SRC_FLOW: &str = "flow-flow";
pub(crate) const SRC_STATION: &str = "flow-station";
pub(crate) const SRC_COMMAND: &str = "flow-command";

#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Flow(String),
    Station(StationId),
    Command(String),
    /// `status` 与 `help`：保留名，说明包或这次运行。
    Info,
}

/// 把一行参数切成词：引号里的算一个词（引号去掉），其余按空白分。
pub(crate) fn tokenize(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    for c in s.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                has = true;
            }
            (None, c) if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (None, c) => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 启动的参数：`--k v` 成输入（`--title` 是运行的标题，覆盖主题）、`--step` 是步进，剩下的词是主题。
#[derive(Debug, Default, PartialEq)]
pub(crate) struct StartArgs {
    pub subject: String,
    pub inputs: serde_json::Map<String, serde_json::Value>,
    pub step: bool,
}

pub(crate) fn start_args(arg: &str) -> StartArgs {
    let toks = tokenize(arg);
    let mut out = StartArgs::default();
    let mut words = vec![];
    let mut title = None;
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        i += 1;
        let Some(key) = t.strip_prefix("--").filter(|k| !k.is_empty()) else {
            words.push(t.clone());
            continue;
        };
        if key == "step" {
            out.step = true;
            continue;
        }
        let (key, value) = match key.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => match toks.get(i) {
                Some(v) if !v.starts_with("--") => {
                    i += 1;
                    (key.to_string(), v.clone())
                }
                _ => (key.to_string(), "true".to_string()),
            },
        };
        if key == "title" {
            title = Some(value);
        } else {
            out.inputs.insert(key, serde_json::Value::String(value));
        }
    }
    out.subject = title.unwrap_or_else(|| words.join(" "));
    out
}

/// 包命令的参数：只认 `--run <运行>`，其余原样（命令自己的 `--flag` 要带过去）。
pub(crate) fn command_args(arg: &str) -> (Vec<String>, Option<String>) {
    let toks = tokenize(arg);
    let mut args = vec![];
    let mut run = None;
    let mut i = 0;
    while i < toks.len() {
        if toks[i] == "--run"
            && let Some(r) = toks.get(i + 1)
        {
            run = Some(r.clone());
            i += 2;
            continue;
        }
        args.push(toks[i].clone());
        i += 1;
    }
    (args, run)
}

fn run_bundle(v: &RunView) -> String {
    v.summary
        .bundle
        .clone()
        .unwrap_or_else(|| v.summary.flow.clone())
}

impl State {
    /// 包的目录项；不在包里的流程合成一项（包名 = 流程名）。
    pub(crate) fn bundle_info(&self, name: &str) -> Option<BundleInfo> {
        if let Some(b) = self.domain.bundles.iter().find(|b| b.name == name) {
            return Some(b.clone());
        }
        let f = self
            .domain
            .flows
            .iter()
            .find(|f| f.name == name && f.bundle.is_none())?;
        Some(BundleInfo {
            name: f.name.clone(),
            title: f.title.clone(),
            flows: vec![],
            commands: vec![],
            stations: self
                .domain
                .flow_details
                .get(name)
                .map(|d| d.stations.iter().map(|s| s.id.clone()).collect())
                .unwrap_or_default(),
            triage: false,
            pace: Pace::Auto,
        })
    }

    /// 流程上下文：左边的对话是哪个运行的会话（主会话、站的会话或站里任务的会话）。
    pub(crate) fn context_run(&self) -> Option<&RunView> {
        let chat = self.ui.chat.as_ref()?;
        self.domain.run_views.values().find(|v| {
            &v.main_session == chat
                || v.stations.iter().any(|st| {
                    st.session.as_ref() == Some(chat)
                        || st.tasks.iter().any(|t| t.session.as_ref() == Some(chat))
                })
        })
    }
}

fn kind_of(cat: &BundleInfo, run: Option<&RunView>, seg: &str) -> Option<Kind> {
    if cat.flows.iter().any(|f| f == seg) {
        return Some(Kind::Flow(seg.to_string()));
    }
    if cat.commands.iter().any(|c| c.name == seg) {
        return Some(Kind::Command(seg.to_string()));
    }
    if matches!(seg, "status" | "help") {
        return Some(Kind::Info);
    }
    if let Some(id) = cat.stations.iter().find(|id| id.0 == seg) {
        return Some(Kind::Station(id.clone()));
    }
    run?.stations
        .iter()
        .find(|st| st.id.0 == seg || st.title == seg)
        .map(|st| Kind::Station(st.id.clone()))
}

/// 提交时先过这里：是流程地址就接下（回命令），不是就放行（`None`）。
pub(crate) fn route(s: &mut State, name: &str, arg: &str) -> Option<Vec<Effect>> {
    if name == "flow" {
        return Some(list_bundles(s));
    }
    if let Some(addr) = name.strip_prefix("flow:") {
        return Some(global(s, addr, arg));
    }
    let (b, seg) = match name.split_once(':') {
        Some((b, seg)) => (b, Some(seg)),
        None => (name, None),
    };
    let ctx = s.context_run().filter(|v| run_bundle(v) == b).cloned();
    let Some(view) = ctx else {
        // 全局里不认简写：交给 Agix，但提醒一句全局的写法（除非它本来就是后端的命令）。
        if seg.is_some()
            && s.domain.bundles.iter().any(|x| x.name == b)
            && !s
                .conv()
                .is_some_and(|c| c.commands.iter().any(|x| x.name == name))
        {
            s.notice(i18n::t!("flow_addr.global_hint", name = name), false);
        }
        return None;
    };
    context(s, &view, b, seg, arg)
}

fn context(
    s: &mut State,
    view: &RunView,
    b: &str,
    seg: Option<&str>,
    arg: &str,
) -> Option<Vec<Effect>> {
    let Some(seg) = seg else {
        status(s, view);
        return Some(vec![]);
    };
    let cat = s.bundle_info(b)?;
    // 包里认不得的名字交给 Agix：同名的插件命令不被盖住。
    let kind = kind_of(&cat, Some(view), seg)?;
    let reason = {
        let r = arg.trim();
        (!r.is_empty()).then(|| r.to_string())
    };
    let title = format!("/{b}:{seg}");
    Some(match kind {
        Kind::Info => {
            status(s, view);
            vec![]
        }
        Kind::Station(station) => vec![s.call(
            Pending::FlowText(title),
            Command::RunOp {
                run: view.summary.id.clone(),
                op: RunOp::Goto { station, reason },
            },
        )],
        Kind::Flow(to) => vec![s.call(
            Pending::FlowText(title),
            Command::RunOp {
                run: view.summary.id.clone(),
                op: RunOp::Switch { to, reason },
            },
        )],
        Kind::Command(command) => {
            let (args, _) = command_args(arg);
            vec![s.call(
                Pending::FlowText(title),
                Command::FlowCommand {
                    bundle: b.to_string(),
                    command,
                    args,
                    run: Some(view.summary.id.clone()),
                },
            )]
        }
    })
}

/// 找不到包时的一句：全局库里有（`library`）就说怎么装，没有就列项目里有的。
pub(crate) fn no_bundle_text(name: &str, names: &str, library: &[String]) -> String {
    if library.iter().any(|n| n == name) {
        i18n::t!("flow_addr.not_installed", name = name)
    } else {
        i18n::t!("flow_addr.no_bundle", name = name, names = names)
    }
}

fn global(s: &mut State, addr: &str, arg: &str) -> Vec<Effect> {
    let (b, seg) = match addr.split_once(':') {
        Some((b, seg)) => (b, Some(seg)),
        None => (addr, None),
    };
    let Some(cat) = s.bundle_info(b) else {
        // 项目里没有：可能在全局库里、没装（flows.md §7.12），问一下实例再说怎么办。
        let names = bundle_names(s).join(&i18n::t!("flows.sep"));
        return vec![s.call(
            Pending::NoBundle {
                name: b.to_string(),
                names,
            },
            Command::ListLibrary,
        )];
    };
    let Some(seg) = seg else {
        if arg.trim().is_empty() {
            describe(s, &cat);
            return vec![];
        }
        return start(s, b, arg, &cat);
    };
    let Some(kind) = kind_of(&cat, None, seg) else {
        s.notice(
            i18n::t!("flow_addr.unknown_name", bundle = b, name = seg),
            true,
        );
        return vec![];
    };
    let title = format!("/flow:{b}:{seg}");
    match kind {
        Kind::Info => {
            describe(s, &cat);
            vec![]
        }
        Kind::Flow(f) => start(s, &format!("{b}/{f}"), arg, &cat),
        Kind::Command(command) => {
            let (args, run) = command_args(arg);
            vec![s.call(
                Pending::FlowText(title),
                Command::FlowCommand {
                    bundle: b.to_string(),
                    command,
                    args,
                    run: run.map(RunId),
                },
            )]
        }
        Kind::Station(station) => {
            let mut words = tokenize(arg);
            let Some(run) = pick_run(s, b, &mut words) else {
                return vec![];
            };
            let reason = words.join(" ");
            vec![s.call(
                Pending::FlowText(title),
                Command::RunOp {
                    run,
                    op: RunOp::Goto {
                        station,
                        reason: (!reason.is_empty()).then_some(reason),
                    },
                },
            )]
        }
    }
}

/// 这个包在跑的运行（摘要）：流程标签展开过的与启动时按包预取的（`ListRuns`）、加上详情里有的。
fn active_runs_of(s: &State, bundle: &str) -> Vec<RunSummary> {
    let mut out: Vec<RunSummary> = vec![];
    let all = s
        .domain
        .runs
        .values()
        .flatten()
        .chain(s.domain.run_views.values().map(|v| &v.summary));
    for r in all {
        let b = r.bundle.as_deref().unwrap_or(&r.flow);
        if b == bundle && !crate::flows::is_ended(&r.status) && !out.iter().any(|x| x.id == r.id) {
            out.push(r.clone());
        }
    }
    out.sort_by(|a, b| a.id.0.cmp(&b.id.0));
    out
}

/// 全局的站操作取哪次运行：第一个词是运行 id 就是它；否则是流程标签选中的（属于这个包的）；
/// 否则是这个包唯一在跑的；都不成就列出候选。
fn pick_run(s: &mut State, bundle: &str, words: &mut Vec<String>) -> Option<RunId> {
    let active = active_runs_of(s, bundle);
    if let Some(w) = words.first()
        && active.iter().any(|r| r.id.0 == *w)
    {
        return Some(RunId(words.remove(0)));
    }
    if let Some(v) = s.selected_run().filter(|v| run_bundle(v) == bundle) {
        return Some(v.summary.id.clone());
    }
    match active.as_slice() {
        [one] => Some(one.id.clone()),
        [] => {
            s.notice(i18n::t!("flow_addr.no_active_run", bundle = bundle), true);
            None
        }
        many => {
            let list: Vec<String> = many
                .iter()
                .map(|r| format!("{} {}", r.id, r.subject))
                .collect();
            s.notice(
                i18n::t!(
                    "flow_addr.which_run",
                    bundle = bundle,
                    runs = list.join(&i18n::t!("flows.sep"))
                ),
                true,
            );
            None
        }
    }
}

fn start(s: &mut State, flow: &str, arg: &str, cat: &BundleInfo) -> Vec<Effect> {
    let a = start_args(arg);
    if a.subject.is_empty() {
        s.notice(i18n::t!("flow_addr.needs_subject", bundle = cat.name), true);
        return vec![];
    }
    vec![s.call(
        Pending::Started,
        Command::StartRun {
            flow: flow.to_string(),
            subject: a.subject,
            inputs: serde_json::Value::Object(a.inputs),
            step: a.step,
            session: s.ui.chat.clone(),
        },
    )]
}

fn bundle_names(s: &State) -> Vec<String> {
    let mut v: Vec<String> = s.domain.bundles.iter().map(|b| b.name.clone()).collect();
    v.extend(
        s.domain
            .flows
            .iter()
            .filter(|f| f.bundle.is_none())
            .map(|f| f.name.clone()),
    );
    v
}

fn show(s: &mut State, title: String, lines: Vec<String>) {
    s.ui.overlay = Some(Overlay::Text {
        title,
        lines,
        scroll: 0,
    });
}

fn list_bundles(s: &mut State) -> Vec<Effect> {
    let lines: Vec<String> = bundle_names(s)
        .iter()
        .filter_map(|n| s.bundle_info(n))
        .map(|b| format!("{}  {}", b.name, b.title))
        .collect();
    show(s, i18n::t!("flow_addr.bundles_title"), lines);
    vec![]
}

/// `/flow:<包>`：包的说明——流程、命令、在跑的运行、用法。
fn describe(s: &mut State, cat: &BundleInfo) {
    let sep = i18n::t!("flows.sep");
    let mut lines = vec![];
    if !cat.flows.is_empty() {
        lines.push(i18n::t!("flow_addr.d_flows", v = cat.flows.join(&sep)));
    }
    if !cat.commands.is_empty() {
        let names: Vec<&str> = cat.commands.iter().map(|c| c.name.as_str()).collect();
        lines.push(i18n::t!("flow_addr.d_commands", v = names.join(&sep)));
    }
    if !cat.stations.is_empty() {
        let names: Vec<&str> = cat.stations.iter().map(|x| x.0.as_str()).collect();
        lines.push(i18n::t!("flow_addr.d_stations", v = names.join(&sep)));
    }
    let runs: Vec<String> = active_runs_of(s, &cat.name)
        .iter()
        .map(|r| format!("  {}  {}  {}", r.id, r.subject, crate::flows::progress(r)))
        .collect();
    if runs.is_empty() {
        lines.push(i18n::t!("flow_addr.d_no_runs"));
    } else {
        lines.push(i18n::t!("flow_addr.d_runs"));
        lines.extend(runs);
    }
    lines.push(String::new());
    lines.push(i18n::t!("flow_addr.d_usage", b = cat.name));
    let title = if cat.title.is_empty() {
        cat.name.clone()
    } else {
        format!("{} ({})", cat.title, cat.name)
    };
    show(s, title, lines);
}

/// 运行此刻：在哪站、欠什么、等谁、几票、有什么建议。
fn status(s: &mut State, v: &RunView) {
    let mut lines = vec![];
    let mut head = format!(
        "{}  {}",
        crate::flows::progress(&v.summary),
        crate::flows::run_status(&v.summary.status).1
    );
    if v.summary.pace == Pace::Step {
        head.push_str(&i18n::t!("flows.pace_step_tag"));
    }
    lines.push(head);
    if let Some(st) = v
        .stations
        .iter()
        .find(|st| !matches!(st.status, StationStatus::Passed | StationStatus::Skipped))
    {
        lines.push(i18n::t!(
            "flow_addr.s_current",
            title = st.title,
            id = st.id,
            state = crate::flows::station_text(st.status)
        ));
        if !st.exit.is_empty() {
            lines.push(i18n::t!("flow_addr.s_exit", v = st.exit.join(", ")));
        }
        if let Some(u) = &st.unmet {
            lines.push(i18n::t!("flow_addr.s_unmet", v = u));
        }
        for c in &st.conditional {
            lines.push(format!("  {}", crate::flows::conditional_text(c)));
        }
        if let Some(vo) = st.votes {
            lines.push(i18n::t!(
                "flow_addr.s_votes",
                n = vo.approvals,
                min = vo.min
            ));
        }
    }
    for p in v
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Open)
    {
        lines.push(proposal_text(p, v));
    }
    if let RunStatus::Waiting { reason } = &v.summary.status {
        lines.push(i18n::t!("flow_addr.s_waiting", v = reason));
    }
    show(s, v.summary.subject.clone(), lines);
}

/// 站写标题（找不到就写 id）；接受的地址里写的是 id。
pub(crate) fn proposal_text(p: &ProposalView, v: &RunView) -> String {
    let title = |id: &StationId| {
        v.stations
            .iter()
            .find(|st| &st.id == id)
            .map_or_else(|| id.0.clone(), |st| st.title.clone())
    };
    i18n::t!(
        "flows.proposal",
        to = title(&p.to),
        station = title(&p.station),
        reason = p.reason,
        by = p.by
    )
}

/// 一次运行的详情到了：新出现的、在等的建议，在这个运行的会话（已打开的）里放一条提示，
/// 正在看的话再弹一句。
pub(crate) fn proposals_arrived(s: &mut State, old: Option<&RunView>, new: &RunView) {
    let fresh: Vec<&ProposalView> = new
        .proposals
        .iter()
        .filter(|p| p.status == ProposalStatus::Open)
        .filter(|p| !old.is_some_and(|o| o.proposals.iter().any(|q| q.id == p.id)))
        .collect();
    if fresh.is_empty() {
        return;
    }
    for p in fresh {
        let text = format!(
            "{}\n{}",
            proposal_text(p, new),
            i18n::t!("flows.proposal_accept", bundle = run_bundle(new), to = p.to)
        );
        let mut sessions: Vec<SessionId> = vec![new.main_session.clone()];
        sessions.extend(
            new.stations
                .iter()
                .filter(|st| st.id == p.station)
                .filter_map(|st| st.session.clone()),
        );
        for sid in sessions {
            if let Some(c) = s.domain.convs.get_mut(&sid) {
                c.push(crate::chat::BlockKind::Notice {
                    text: text.clone(),
                    tone: tui::Tone::Warn,
                });
            }
        }
        if s.context_run()
            .is_some_and(|v| v.summary.id == new.summary.id)
        {
            s.notice(proposal_text(p, new), false);
        }
    }
}

/// 补全：`/flow:` 下是包，`/flow:<包>:` 下是它的流程、命令、站；流程上下文里 `/<包>:` 同理。
/// `prefix` 是 `/` 后面已打的部分；不是地址就回 `None`，由调用方走普通命令补全。
pub(crate) fn candidates(s: &State, prefix: &str) -> Option<Vec<SlashCommand>> {
    let mk = |name: String, description: String, source: &str| SlashCommand {
        name,
        description,
        source: source.into(),
    };
    if let Some(rest) = prefix.strip_prefix("flow:") {
        return Some(match rest.split_once(':') {
            None => bundle_names(s)
                .iter()
                .filter_map(|n| s.bundle_info(n))
                .filter(|b| b.name.starts_with(rest))
                .map(|b| mk(format!("flow:{}", b.name), b.title, SRC_BUNDLE))
                .collect(),
            Some((b, tail)) => {
                let cat = s.bundle_info(b)?;
                members(&cat, None, tail, &format!("flow:{b}:"))
            }
        });
    }
    let (b, tail) = prefix.split_once(':')?;
    let view = s.context_run().filter(|v| run_bundle(v) == b)?;
    let cat = s.bundle_info(b)?;
    Some(members(&cat, Some(view), tail, &format!("{b}:")))
}

fn members(cat: &BundleInfo, run: Option<&RunView>, tail: &str, head: &str) -> Vec<SlashCommand> {
    let mut out = vec![];
    let mut add = |name: &str, desc: String, src: &str| {
        if name.starts_with(tail) {
            out.push(SlashCommand {
                name: format!("{head}{name}"),
                description: desc,
                source: src.into(),
            });
        }
    };
    for f in &cat.flows {
        add(f, String::new(), SRC_FLOW);
    }
    for c in &cat.commands {
        add(&c.name, c.description.clone(), SRC_COMMAND);
    }
    let mut seen: Vec<&StationId> = cat.stations.iter().collect();
    if let Some(v) = run {
        for st in &v.stations {
            if !seen.contains(&&st.id) {
                seen.push(&st.id);
            }
        }
    }
    for id in seen {
        let title = run
            .and_then(|v| v.stations.iter().find(|st| &st.id == id))
            .map(|st| st.title.clone())
            .unwrap_or_default();
        add(&id.0, title, SRC_STATION);
    }
    out
}

/// 补全候选里的种类标签。
pub(crate) fn kind_label(source: &str) -> Option<String> {
    Some(match source {
        SRC_BUNDLE => i18n::t!("flow_addr.kind_bundle"),
        SRC_FLOW => i18n::t!("flow_addr.kind_flow"),
        SRC_STATION => i18n::t!("flow_addr.kind_station"),
        SRC_COMMAND => i18n::t!("flow_addr.kind_command"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_strings_stay_one_word() {
        assert_eq!(
            tokenize(r#"a "b c" 'd e' f"#),
            ["a", "b c", "d e", "f"].map(String::from)
        );
        assert_eq!(tokenize(r#"x "" y"#), ["x", "", "y"].map(String::from));
    }

    #[test]
    fn start_args_split_inputs_flag_and_subject() {
        let a = start_args(r#"登录加 OAuth --risk low --step --title "加 OAuth" --dry"#);
        assert!(a.step);
        assert_eq!(a.subject, "加 OAuth");
        assert_eq!(a.inputs["risk"], "low");
        assert_eq!(a.inputs["dry"], "true");
        let b = start_args("导出 报表 --risk=medium");
        assert_eq!(b.subject, "导出 报表");
        assert_eq!(b.inputs["risk"], "medium");
        assert!(!b.step);
    }

    #[test]
    fn command_args_keep_the_commands_own_flags() {
        let (args, run) = command_args(r#"delta --change "add oauth" --run x1"#);
        assert_eq!(args, ["delta", "--change", "add oauth"].map(String::from));
        assert_eq!(run.as_deref(), Some("x1"));
    }
}
