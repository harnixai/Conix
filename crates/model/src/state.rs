use std::collections::HashMap;
use std::sync::Arc;

use keys::{Context, Keymap};
use std::collections::HashMap as Map;

use port::{
    ClientInfo, ConnState, FlowSummary, InstanceInfo, Job, PermissionMode, PromptId, SessionId,
    SessionSummary,
};

use crate::chat::Conversation;
use crate::composer::ComposerState;
use tui::{ShowPane, Size};

use crate::update::{Pending, ReqId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Sessions,
    Flows,
    Jobs,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Sessions, Tab::Flows, Tab::Jobs];

    pub fn label(self) -> String {
        match self {
            Tab::Sessions => i18n::t!("state.tab.sessions"),
            Tab::Flows => i18n::t!("state.tab.flows"),
            Tab::Jobs => i18n::t!("state.tab.jobs"),
        }
    }

    pub fn context(self) -> Context {
        match self {
            Tab::Sessions => Context::Sessions,
            Tab::Flows => Context::Flows,
            Tab::Jobs => Context::Jobs,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Tab::Sessions => 0,
            Tab::Flows => 1,
            Tab::Jobs => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Composer,
    Transcript,
    SideUpper,
    SideLower,
}

impl Focus {
    pub fn in_side(self) -> bool {
        matches!(self, Focus::SideUpper | Focus::SideLower)
    }
}

/// 整屏显示什么（交互 §2.1、§2.2、§9）。
#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    Instances(Picker),
    Workbench,
    /// 已脱离：实例还在跑，这个 TUI 没接着它。
    Detached(DetachReason),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Picker {
    pub list: Vec<InstanceInfo>,
    /// 选中行：0 是第一行"＋ 新建实例"，`i` 是 `list[i - 1]`。
    pub selected: usize,
    /// 视口第一行（一次只显示 `PICKER_ROWS` 行，交互 §2.2）。
    pub scroll: usize,
    pub loading: bool,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DetachReason {
    Kicked { by: ClientInfo, at_ms: u64 },
    Idle { after_ms: u64 },
    Stopped { reason: String },
}

/// 底栏上的二次确认（交互 §5.1）：`y` / Enter 确认，其他键放弃。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    ForceAttach(String),
    StopInstance(String),
    CloseSession(SessionId, String),
    StopTask(port::TaskId, String),
    ClearGoal(SessionId),
    CancelRun(port::RunId, String),
    /// 停下作业正在跑的那一次：作业、哪一次（省略 = 正在跑的那次）、名字。
    StopFiring(port::JobId, Option<u64>, String),
    /// 删掉一个已关闭的会话：会话、名字。
    DeleteSession(SessionId, String),
    ForceStation(port::RunId, port::StationId, String),
    Rework(port::RunId, port::StationId, String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Overlay {
    Help {
        scroll: usize,
    },
    /// 退出时的选择：后台运行 / 直接退出 / 返回（交互 §7）。
    Exit {
        selected: usize,
    },
    /// 只读的文字浮层：流程的原始 YAML。
    Text {
        title: String,
        lines: Vec<String>,
        scroll: usize,
    },
    /// `/events`：最近的事件与命令。
    EventLog {
        scroll: usize,
    },
    /// 输入框为空时连按两次 Esc：选一轮退回去，把那一轮的话放回输入框（交互 §4.5）。
    Rewind {
        session: SessionId,
        turns: Vec<port::Checkpoint>,
        selected: usize,
    },
    /// `/model` 不带参数：选一个模型。
    Models {
        models: Vec<port::ModelInfo>,
        selected: usize,
    },
    /// `/plugin`：装好的在前、可装的包在后。`armed` 是按过一次 d、等再按一次才卸的那个插件。
    Plugins {
        plugins: port::Plugins,
        selected: usize,
        armed: Option<String>,
    },
    /// 插件的两份披露：装完等人确认（`installing`），或随时再看（只读）。
    PluginDisclosure {
        plugin: Box<port::PluginInfo>,
        disclosure: Box<port::PluginDisclosure>,
        scroll: usize,
        installing: bool,
    },
    /// 接入时：离开期间攒下的等人处理的事（交互 §5.5）。
    Away {
        since_ms: u64,
        alerts: Vec<port::Alert>,
        selected: usize,
    },
}

/// 一个标签自己的选中与滚动；切标签再切回来保持原样。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SectionUi {
    pub selected: usize,
    pub scroll: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drag {
    Divider,
}

#[derive(Debug, Clone)]
pub struct Notice {
    pub text: String,
    pub error: bool,
    pub until_ms: u64,
}

/// 后端事实：只由事件与命令结果写入。
#[derive(Debug, Clone, Default)]
pub struct Domain {
    pub instance: Option<InstanceInfo>,
    pub flows_available: bool,
    pub jobs_available: bool,
    pub conn: Option<ConnState>,
    pub sessions: Vec<SessionSummary>,
    pub flows: Vec<FlowSummary>,
    pub jobs: Vec<Job>,
    /// 被 Watch 的会话。
    pub convs: Map<SessionId, Conversation>,
    /// 任务到目前为止的输出（看任务输出时取，之后 `TaskOutput` 事件追加）。
    pub task_output: Map<port::TaskId, String>,
    /// 每个流程的运行（展开流程时取）。
    pub runs: Map<String, Vec<port::RunSummary>>,
    pub run_views: Map<port::RunId, port::RunView>,
    /// 项目里的文件（`@` 提及的补全）；`None` 是还没取。
    pub project_files: Option<Vec<String>>,
    /// 在 Conix 里关掉的会话（会话标签末尾的「已关闭」）。
    pub closed_sessions: Vec<SessionSummary>,
    pub closed_loaded: bool,
    /// 已结束的运行，不分流程，结束时刻新的在前（流程标签末尾的「已结束」，交互 §5.3）。
    pub ended_runs: Vec<port::RunSummary>,
    /// 取过一次了（取失败也算，免得每帧重取）。
    pub ended_loaded: bool,
    /// 上一次取满了一页：后面可能还有，列表末尾放「更多」。
    pub ended_more: bool,
    pub flow_details: Map<String, port::FlowDetail>,
    pub job_history: Map<port::JobId, Vec<port::JobEntry>>,
    /// 列表加载失败的原因，按标签（会话 / 流程 / 作业）。
    pub load_errors: [Option<String>; 3],
}

/// 界面状态：只由输入与 Tick 写入。
#[derive(Debug, Clone)]
pub struct Ui {
    pub screen: Screen,
    pub confirm: Option<Confirm>,
    /// 接着的实例名（工作台或"已脱离"时有）。
    pub attached: Option<String>,
    pub last_input_ms: u64,
    /// 0 表示不超时（需求 I-10）。
    pub idle_timeout_ms: u64,
    pub idle_warned: bool,
    pub next_heartbeat_ms: u64,
    pub size: Size,
    pub now_ms: u64,
    pub ratio_pct: u16,
    pub side_collapsed: bool,
    /// 单栏模式下显示哪一栏。
    pub showing: ShowPane,
    pub focus: Focus,
    pub tab: Tab,
    pub upper: [SectionUi; 3],
    pub lower: [SectionUi; 3],
    pub overlay: Option<Overlay>,
    pub mouse_captured: bool,
    pub drag: Option<Drag>,
    pub notice: Option<Notice>,
    /// 空闲时第一次 Ctrl+C 的时刻：两秒内再按一次才脱离。
    pub quit_armed_ms: Option<u64>,
    pub last_click: Option<(u16, u16, u64)>,
    /// 对话区当前显示的会话。
    pub chat: Option<SessionId>,
    pub composer: ComposerState,
    /// 转录详细（Ctrl+O 看转录时打开，回输入框时关上）。
    pub detail: bool,
    /// Ctrl+T 收放过的对话区顶部面板（按会话记）；没记的按有没有内容决定。
    pub panel: std::collections::HashMap<SessionId, bool>,
    /// 面板滚到第几行，连同当时的上下文；上下文一换就回到顶上。
    pub panel_scroll: (String, usize),
    /// 输入框手动滚到的位置，连同当时的（光标、文字长度）：一编辑就回到跟着光标走。
    pub composer_scroll: Option<((usize, usize), usize)>,
    /// 草稿里 Shift+Tab 选的权限模式（建会话时带上）；打开了会话时显示与切换的是会话自己的。
    pub permission: PermissionMode,
    /// 上一次 Shift+Tab 请求的模式被设置夹窄了：下一次从请求的那个往后轮，不然缺省设置下
    /// default → acceptEdits（夹回 default）→ acceptEdits……永远轮不到 plan。
    pub permission_asked: Option<(SessionId, PermissionMode)>,
    /// 提问框拿着键盘（交互 §3：输入框为空时提问来了直接拿，否则按 Ctrl+R / 点它）。
    pub ask_active: bool,
    pub ask_selected: usize,
    /// 正在用输入框自由回答的提问。
    pub answering: Option<(SessionId, PromptId)>,
    pub esc_armed_ms: Option<u64>,
    pub detach_armed_ms: Option<u64>,
    /// 测试用计数：重排了多少个块（testing.md §1 用计数验性能）。
    pub relayouts: u64,
    pub quitting: bool,
    /// 会话标签：展开了的主 Agent、展开了 todo 明细的、收起来的子 Agent、展开了"已结束"的。
    pub sess_expanded: std::collections::HashSet<SessionId>,
    pub todo_expanded: std::collections::HashSet<SessionId>,
    pub tree_collapsed: std::collections::HashSet<port::TaskId>,
    pub finished_expanded: std::collections::HashSet<SessionId>,
    /// 已发出 Watch、还没回来的。
    pub watching: std::collections::HashSet<SessionId>,
    /// 快照还没回来的会话先到的事件：快照回来后按序补上。实例只推快照之后的事件，但它们
    /// 可能比快照的回复先到（两条 channel），丢了就少几个字。
    pub watch_buffer: std::collections::HashMap<SessionId, Vec<port::Event>>,
    /// 对话区的上下文栈（面包屑）：进入子 Agent 或任务输出之前的那些会话与标题。
    pub ctx_parents: Vec<(SessionId, String)>,
    /// 对话区是子 Agent 的侧链会话时，它对应的任务：输入的话经"给任务发消息"送达（C-16）。
    pub via_task: Option<port::TaskId>,
    /// 对话区在看某个任务的输出（只读）。
    pub task_view: Option<port::TaskId>,
    /// 正在给这个会话改名：输入框里是新名字（交互 §5.2 `r`）。
    pub rename: Option<SessionId>,
    pub flow_expanded: std::collections::HashSet<String>,
    /// ^R / "离开期间"要跳去的那一行还没取回来：数据到了再选中（`follow::land`）。
    pub pending_jump: Option<crate::follow::Attention>,
    /// 待跳转已经切过一次标签：之后标签变了就是人自己换走了。
    pub pending_jump_set: bool,
    /// 会话标签的「已关闭」展开着（缺省收起）。
    pub closed_expanded: bool,
    /// 「已结束」展开着（缺省收起）。
    pub ended_expanded: bool,
    /// 上一次提示过"运行已结束"的运行：同一个只提示一次。
    pub ended_noticed: Option<port::RunId>,
    /// 运行里收起的站（`运行/站`）；有子 Agent 的站缺省展开。
    pub station_collapsed: std::collections::HashSet<String>,
    /// 上一次因为选中而自动展开的流程：同一个流程被 ← 收起后不再自动展开。
    pub flow_auto: Option<String>,
    /// 上一次"选中即切换"的依据（选中的是哪个对象）；选中没变就不再切。
    pub last_follow: Option<String>,
    /// 右栏的选中按对象记（`[标签][上半 0 / 下半 1]` → 上次确认时的行号与那一行的身份）：列表
    /// 插入、删除、重排、展开收起之后把选中挪回同一个对象。`None` 表示刚被显式改过，照新行号认。
    pub anchors: [[Option<(usize, String)>; 2]; 3],
    /// 左边换了会话、右栏选中已经对上的那个会话（`Some(None)` 是草稿）；没对上时每次 update
    /// 再试一次，好等列表里出现它（比如刚建的会话，回复先于列表更新到）。
    pub revealed_for: Option<Option<SessionId>>,
    /// 各标签的过滤字（`/`），以及是否正在输入过滤字。
    pub filter: [String; 3],
    pub filtering: bool,
    /// 已发出、还没回来的加载（避免重复请求）。
    pub loading: std::collections::HashSet<String>,
    /// 最近的事件与命令（`/events` 事件日志）。
    pub event_log: std::collections::VecDeque<String>,
    /// Ctrl+R 上次跳到第几处。
    pub attention_cursor: usize,
}

impl Ui {
    pub fn upper(&self) -> &SectionUi {
        &self.upper[self.tab.index()]
    }

    pub fn upper_mut(&mut self) -> &mut SectionUi {
        &mut self.upper[self.tab.index()]
    }

    pub fn lower_mut(&mut self) -> &mut SectionUi {
        &mut self.lower[self.tab.index()]
    }

    pub fn lower(&self) -> &SectionUi {
        &self.lower[self.tab.index()]
    }
}

pub const PICKER_ROWS: usize = 6;
pub const DEFAULT_IDLE_TIMEOUT_MS: u64 = 30 * 60_000;
pub const HEARTBEAT_MS: u64 = 10_000;
pub const IDLE_WARN_MS: u64 = 60_000;

pub struct State {
    pub settings: crate::Settings,
    pub domain: Domain,
    pub ui: Ui,
    pub keymap: Arc<Keymap>,
    pub(crate) pending: HashMap<ReqId, Pending>,
    pub(crate) next_req: u64,
}

impl State {
    pub fn new(keymap: Arc<Keymap>, size: Size, now_ms: u64) -> State {
        State::with_settings(keymap, size, now_ms, crate::Settings::default())
    }

    pub fn with_settings(
        keymap: Arc<Keymap>,
        size: Size,
        now_ms: u64,
        settings: crate::Settings,
    ) -> State {
        let ratio = settings.ratio_pct.clamp(40, 75);
        let idle = settings.idle_timeout_ms;
        State {
            settings,
            domain: Domain::default(),
            ui: Ui {
                screen: Screen::Workbench,
                confirm: None,
                attached: None,
                last_input_ms: now_ms,
                idle_timeout_ms: idle,
                idle_warned: false,
                next_heartbeat_ms: now_ms + HEARTBEAT_MS,
                size,
                now_ms,
                ratio_pct: ratio,
                side_collapsed: false,
                showing: ShowPane::Chat,
                focus: Focus::Composer,
                tab: Tab::Sessions,
                upper: Default::default(),
                lower: Default::default(),
                overlay: None,
                mouse_captured: true,
                drag: None,
                notice: None,
                quit_armed_ms: None,
                last_click: None,
                chat: None,
                composer: ComposerState::default(),
                detail: false,
                panel: Default::default(),
                panel_scroll: Default::default(),
                composer_scroll: None,
                permission: PermissionMode::Default,
                permission_asked: None,
                ask_active: false,
                ask_selected: 0,
                answering: None,
                esc_armed_ms: None,
                detach_armed_ms: None,
                relayouts: 0,
                quitting: false,
                sess_expanded: Default::default(),
                todo_expanded: Default::default(),
                tree_collapsed: Default::default(),
                finished_expanded: Default::default(),
                watching: Default::default(),
                watch_buffer: Default::default(),
                ctx_parents: vec![],
                via_task: None,
                task_view: None,
                rename: None,
                flow_expanded: Default::default(),
                ended_expanded: false,
                closed_expanded: false,
                pending_jump: None,
                pending_jump_set: false,
                ended_noticed: None,
                station_collapsed: Default::default(),
                flow_auto: None,
                last_follow: None,
                anchors: Default::default(),
                revealed_for: None,
                filter: Default::default(),
                filtering: false,
                loading: Default::default(),
                event_log: Default::default(),
                attention_cursor: 0,
            },
            keymap,
            pending: HashMap::new(),
            next_req: 1,
        }
    }

    /// 键位解析用的上下文，从最具体到最宽（`Global` 由解析器自己补上）。
    pub fn key_contexts(&self) -> Vec<Context> {
        if self.ui.confirm.is_some() {
            return vec![Context::Dialog];
        }
        if self.ui.overlay.is_some() {
            return vec![Context::Overlay];
        }
        if !matches!(self.ui.screen, Screen::Workbench) {
            return vec![Context::Instances];
        }
        if self.ui.filtering {
            return vec![Context::Picker];
        }
        match self.ui.focus {
            Focus::Composer
                if self.ask_visible() && self.ui.ask_active && self.ui.task_view.is_none() =>
            {
                vec![Context::Dialog]
            }
            Focus::Composer if !self.picker_candidates().is_empty() => {
                vec![Context::Picker, Context::Composer]
            }
            Focus::Composer => vec![Context::Composer],
            Focus::Transcript => vec![Context::Transcript],
            Focus::SideUpper | Focus::SideLower => vec![self.ui.tab.context()],
        }
    }

    pub fn conv(&self) -> Option<&Conversation> {
        self.ui
            .chat
            .as_ref()
            .and_then(|id| self.domain.convs.get(id))
    }

    pub fn conv_mut(&mut self) -> Option<&mut Conversation> {
        let id = self.ui.chat.clone()?;
        self.domain.convs.get_mut(&id)
    }

    /// Shift+Tab 切的是哪个会话：对话区里的会话；在子 Agent 里时是它的根会话（模式随根会话）。
    /// 看任务输出、草稿时没有。
    pub fn permission_session(&self) -> Option<SessionId> {
        if self.ui.task_view.is_some() {
            return None;
        }
        if self.ui.via_task.is_some() {
            return self.ui.ctx_parents.first().map(|(id, _)| id.clone());
        }
        self.ui.chat.clone()
    }

    /// 输入框下显示的权限模式：会话的（以最新的摘要为准）；草稿时是本地选的。
    pub fn permission_shown(&self) -> PermissionMode {
        match self.permission_session() {
            Some(id) => self
                .domain
                .sessions
                .iter()
                .find(|x| x.id == id)
                .map(|x| x.permission)
                .or_else(|| self.domain.convs.get(&id).map(|c| c.summary.permission))
                .unwrap_or_default(),
            None => self.ui.permission,
        }
    }

    pub fn ask_visible(&self) -> bool {
        self.conv().is_some_and(|c| !c.prompts.is_empty())
    }

    /// 右栏会话列表只列主 Agent；子 Agent 的侧链会话挂在它的拆分树下（需求 S-01、S-04）。
    pub fn main_sessions(&self) -> impl Iterator<Item = &SessionSummary> {
        self.domain
            .sessions
            .iter()
            .filter(|s| !matches!(s.provenance.origin, port::Origin::SubAgent { .. }))
    }

    /// 会话标签列的会话：人发起的（新建、分叉），按规范顺序（`port::order`），`#n` 就是它的位置。
    /// 流程和作业开出来的会话在流程、作业标签里找。
    pub fn user_sessions(&self) -> Vec<&SessionSummary> {
        port::order::sessions(&self.domain.sessions)
    }

    /// 最近活跃的用户会话（会话标签的落点、流程 / 作业标签的 `n` `e` `d` 回到哪里）。
    pub fn recent_user_session(&self) -> Option<SessionId> {
        self.user_sessions()
            .into_iter()
            .max_by_key(|s| s.updated_ms)
            .map(|s| s.id.clone())
    }

    /// 左边是不是一个用户会话（不是运行、作业、子 Agent 的会话，也不是草稿）。
    pub fn chat_is_user_session(&self) -> bool {
        self.ui.chat.as_ref().is_some_and(|id| {
            self.domain
                .sessions
                .iter()
                .any(|s| &s.id == id && port::order::is_user_session(s))
        })
    }

    pub fn wide(&self) -> bool {
        self.ui.size.cols >= tui::layout::WIDE_MIN_COLS
    }

    /// 当前右栏上半的行数（2.5 起由各标签自己的模型给出）。
    pub fn lower_len_pub(&self) -> usize {
        self.lower_len()
    }

    pub(crate) fn lower_len(&self) -> usize {
        match self.ui.tab {
            Tab::Sessions => self.tree_rows().len(),
            Tab::Flows => self.flow_lower_rows().len(),
            Tab::Jobs => self.job_lower_rows().len(),
        }
    }

    pub(crate) fn upper_len(&self) -> usize {
        match self.ui.tab {
            Tab::Sessions => self.session_rows().len(),
            Tab::Flows => self.flow_rows().len(),
            Tab::Jobs => self.job_rows().len(),
        }
    }
}
