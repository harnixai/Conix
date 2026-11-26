//! 插件（docs/design/plugins.md）：Agix 管装、卸、启停与包里的命令、函数、脚本；Conix 读包里 `[x-conix]`
//! 这一段（Agix 不读 `x-` 段）——流程包在哪、门禁与程序步骤各由哪个插件函数做——并把流程包挂进流程库。

pub mod gates;
pub mod hostlib;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

/// 包里 `plugin.toml` 的 `[x-conix]`。
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct ConixSection {
    /// 流程包目录（相对包根），布局同 harnix-flow 的 `DirPacks`。
    #[serde(default)]
    pub flows: Option<String>,
    /// host 门禁名 → 插件函数的调用名（`<包名>.<函数名>`），或 `{ function, runs_commands = true }`。
    #[serde(default)]
    pub gates: BTreeMap<String, GateTarget>,
    /// program 步骤名 → 插件函数的调用名，或 `{ function, timeout = "5m" }`。
    #[serde(default)]
    pub steps: BTreeMap<String, StepTarget>,
    /// 受管的流程（design/flows.md §5）：`true` 是包里全部流程，或者列出名字。受管的流程与包里所有的件，项目里
    /// 同名的不顶替、存不进去；它的运行不能强制放行。
    #[serde(default)]
    pub governed: Governed,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Governed {
    #[default]
    #[serde(skip)]
    None,
    All(bool),
    Flows(Vec<String>),
}

impl Governed {
    /// 交给流程库的形状（`DirPacks::with_governed`）：不受管是 `None`；整包是 `Some(None)`。
    pub fn flows(&self) -> Option<Option<Vec<String>>> {
        match self {
            Governed::None | Governed::All(false) => None,
            Governed::All(true) => Some(None),
            Governed::Flows(f) if f.is_empty() => None,
            Governed::Flows(f) => Some(Some(f.clone())),
        }
    }
}

/// 门禁与步骤调插件函数的缺省时限（`plugin.call` 的缺省）。
pub const CALL_TIMEOUT_MS: u64 = 120_000;
/// 能声明的最长时限：Agix 给宿主调的 `plugin.call` 至多十分钟（`plugin_call_long`，与 `Bash` 的上限一致）。
pub const MAX_CALL_TIMEOUT_MS: u64 = 600_000;

/// `timeout = "5m"`（`90s`、`1h30m`，纯数字按分钟）→ 毫秒；没写、写错的用缺省，长过十分钟的按十分钟。
fn call_timeout(t: Option<&str>) -> u64 {
    match t {
        None => CALL_TIMEOUT_MS,
        Some(s) => match crate::jobs::guard::parse_duration(s) {
            Some(ms) => ms.min(MAX_CALL_TIMEOUT_MS),
            None => {
                tracing::warn!(
                    timeout = s,
                    "plugin.toml: not a duration, using the default"
                );
                CALL_TIMEOUT_MS
            }
        },
    }
}

/// 一个程序步骤由哪个函数做、给它多久。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum StepTarget {
    Function(String),
    Full {
        function: String,
        #[serde(default)]
        timeout: Option<String>,
    },
}

impl StepTarget {
    pub fn function(&self) -> &str {
        match self {
            StepTarget::Function(f) | StepTarget::Full { function: f, .. } => f,
        }
    }

    pub fn timeout_ms(&self) -> u64 {
        match self {
            StepTarget::Function(_) => CALL_TIMEOUT_MS,
            StepTarget::Full { timeout, .. } => call_timeout(timeout.as_deref()),
        }
    }
}

/// 一道门禁由哪个函数判。缺省以 `writes:false` 调：只读的命令（`git status`）不用规则就能跑，会改东西的一律被拒。
/// 要跑测试这类命令的门禁写 `runs_commands = true`，以 `writes:true` 调：经文件工具仍只写得了函数声明的
/// `writes`，命令按项目设置的规则判（不带会话，要问人的失败）。安装时披露里写明。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum GateTarget {
    Function(String),
    Full {
        function: String,
        #[serde(default)]
        runs_commands: bool,
        /// 跑测试这类门禁两分钟不够时写，如 `"8m"`；至多十分钟。
        #[serde(default)]
        timeout: Option<String>,
    },
}

impl GateTarget {
    pub fn function(&self) -> &str {
        match self {
            GateTarget::Function(f) | GateTarget::Full { function: f, .. } => f,
        }
    }

    pub fn timeout_ms(&self) -> u64 {
        match self {
            GateTarget::Function(_) => CALL_TIMEOUT_MS,
            GateTarget::Full { timeout, .. } => call_timeout(timeout.as_deref()),
        }
    }

    pub fn runs_commands(&self) -> bool {
        matches!(
            self,
            GateTarget::Full {
                runs_commands: true,
                ..
            }
        )
    }
}

#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default, rename = "x-conix")]
    x_conix: Option<ConixSection>,
}

/// 读包根上 `plugin.toml` 的 `[x-conix]`；没有这一段、读不了的回 `None`（包照样是个 Agix 插件）。
pub fn conix_section(root: &Path) -> Option<ConixSection> {
    let text = std::fs::read_to_string(root.join("plugin.toml")).ok()?;
    match toml::from_str::<Manifest>(&text) {
        Ok(m) => m.x_conix,
        Err(e) => {
            tracing::warn!(root = %root.display(), "plugin.toml: {e}");
            None
        }
    }
}

/// 一个启用着、带 `[x-conix]` 的插件。
#[derive(Debug, Clone, PartialEq)]
pub struct Mounted {
    pub name: String,
    pub root: PathBuf,
    pub section: ConixSection,
}

impl Mounted {
    /// 流程包的目录；不在包里（`..`、绝对路径、出包的链接）的不挂——包是别人写的。
    pub fn flows_dir(&self) -> Option<PathBuf> {
        let rel = self.section.flows.as_deref()?;
        let p = Path::new(rel);
        if p.is_absolute()
            || p.components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return None;
        }
        let dir = self.root.join(p).canonicalize().ok()?;
        let root = self.root.canonicalize().ok()?;
        (dir.starts_with(&root) && dir.is_dir()).then_some(dir)
    }

    /// 流程包里的流程名（`flows/*.yaml` 的文件名）。
    pub fn flow_names(&self) -> Vec<String> {
        let Some(dir) = self.flows_dir() else {
            return vec![];
        };
        let mut names: Vec<String> = std::fs::read_dir(dir.join("flows"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                matches!(p.extension().and_then(|x| x.to_str()), Some("yaml" | "yml"))
                    .then(|| p.file_stem()?.to_str().map(str::to_string))
                    .flatten()
            })
            .collect();
        names.sort();
        names
    }
}

/// 所有启用着的插件的门禁与步骤：名字 → (插件, 调用名)。同名的按插件名先到的胜出，后到的记成冲突。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginMap {
    pub gates: BTreeMap<String, (String, String)>,
    pub steps: BTreeMap<String, (String, String)>,
    /// 以 `writes:true` 调的门禁（[`GateTarget::runs_commands`]）。
    pub runs_commands: std::collections::BTreeSet<String>,
    /// 声明了时限的门禁与步骤：名字 → 毫秒；没有的用 [`CALL_TIMEOUT_MS`]。
    pub gate_timeouts: BTreeMap<String, u64>,
    pub step_timeouts: BTreeMap<String, u64>,
    /// 插件名 → 被遮住的东西，一条一句。
    pub conflicts: BTreeMap<String, Vec<String>>,
}

impl PluginMap {
    pub fn build(mounted: &[Mounted]) -> PluginMap {
        let mut sorted: Vec<&Mounted> = mounted.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        let mut m = PluginMap::default();
        for p in sorted {
            let gates: BTreeMap<String, String> = p
                .section
                .gates
                .iter()
                .map(|(g, t)| (g.clone(), t.function().to_string()))
                .collect();
            let steps: BTreeMap<String, String> = p
                .section
                .steps
                .iter()
                .map(|(n, t)| (n.clone(), t.function().to_string()))
                .collect();
            // 门禁、步骤的附加项只算胜出的那个插件的（下面同名的后到者记成冲突）。
            for (g, t) in &p.section.gates {
                if !m.gates.contains_key(g) {
                    if t.runs_commands() {
                        m.runs_commands.insert(g.clone());
                    }
                    if t.timeout_ms() != CALL_TIMEOUT_MS {
                        m.gate_timeouts.insert(g.clone(), t.timeout_ms());
                    }
                }
            }
            for (n, t) in &p.section.steps {
                if !m.steps.contains_key(n) && t.timeout_ms() != CALL_TIMEOUT_MS {
                    m.step_timeouts.insert(n.clone(), t.timeout_ms());
                }
            }
            for (kind, src, dst) in [
                ("gate", &gates, &mut m.gates),
                ("step", &steps, &mut m.steps),
            ] {
                for (name, function) in src {
                    match dst.get(name) {
                        Some((by, _)) => m
                            .conflicts
                            .entry(p.name.clone())
                            .or_default()
                            .push(format!("{kind} `{name}` is taken by plugin `{by}`")),
                        None => {
                            dst.insert(name.clone(), (p.name.clone(), function.clone()));
                        }
                    }
                }
            }
        }
        m
    }
}

/// 可装的包：目录里的 `<名>-<版本>.zip`，只按文件名认，不解包（内容以 Agix 装时读出的为准）。
pub fn packages(dirs: &[PathBuf]) -> Vec<port::PluginPackage> {
    let mut out: Vec<port::PluginPackage> = vec![];
    for d in dirs {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let path = e.path();
            let Some(file) = path.file_name().and_then(|f| f.to_str()) else {
                continue;
            };
            let Some((name, version)) = split_package_name(file) else {
                continue;
            };
            // 同名同版本在前面的目录里已经有了：先到的为准（自带目录排在用户的前面）。
            if out.iter().any(|p| p.name == name && p.version == version) {
                continue;
            }
            out.push(port::PluginPackage {
                name,
                version,
                path: path.to_string_lossy().into_owned(),
                installed: false,
            });
        }
    }
    out.sort_by(|a, b| (&a.name, &a.version).cmp(&(&b.name, &b.version)));
    out
}

/// `xforge-0.1.0.zip` → (`xforge`, `0.1.0`)：版本是最后一个后面跟数字的 `-` 之后的部分。
pub fn split_package_name(file: &str) -> Option<(String, String)> {
    let stem = file.strip_suffix(".zip")?;
    let i = stem
        .char_indices()
        .filter(|(_, c)| *c == '-')
        .map(|(i, _)| i)
        .rfind(|i| stem[i + 1..].starts_with(|c: char| c.is_ascii_digit()))?;
    let (name, version) = (&stem[..i], &stem[i + 1..]);
    (!name.is_empty() && !version.is_empty()).then(|| (name.to_string(), version.to_string()))
}

/// zip 的 SHA-256（`plugin.install` 的 `checksum`）。
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)?;
    Ok(Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// 自带插件目录：`CONIX_PLUGIN_DIRS`（`:` 分隔，开发与测试用）替换 `<exe 所在目录>/../share/conix/plugins/`；
/// 用户自己放包的 `<家目录>/plugin-store/` 总在后面。
pub fn package_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = match std::env::var_os("CONIX_PLUGIN_DIRS") {
        Some(v) => std::env::split_paths(&v).collect(),
        None => std::env::current_exe()
            .ok()
            .and_then(|e| Some(e.parent()?.join("../share/conix/plugins")))
            .into_iter()
            .collect(),
    };
    dirs.push(home.join("plugin-store"));
    dirs
}

/// Agix `plugin.install` 回的 `disclosure` 加上 Conix 自己的那份（`[x-conix]` 带来的，Agix 不读）。
pub fn disclosure(agix: &Value, mounted: Option<&Mounted>) -> port::PluginDisclosure {
    let strings = |v: &Value| -> Vec<String> {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect()
    };
    let mut d = port::PluginDisclosure {
        capabilities: strings(&agix["capabilities"]),
        mcp_servers: strings(&agix["mcp_servers"]),
        model_visible: agix["model_visible"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|x| port::VisibleText {
                origin: x["origin"].as_str().unwrap_or_default().to_string(),
                text: x["text"].as_str().unwrap_or_default().to_string(),
            })
            .collect(),
        ..Default::default()
    };
    if let Some(m) = mounted {
        conix_disclosure(m, &mut d);
    }
    d
}

/// 披露里写明比缺省的两分钟长的时限：一道门禁最多能占着一站这么久。
fn longer(ms: u64) -> String {
    if ms <= CALL_TIMEOUT_MS {
        return String::new();
    }
    let s = ms / 1000;
    match (s / 60, s % 60) {
        (m, 0) => format!(", up to {m} min"),
        (m, r) => format!(", up to {m} min {r} s"),
    }
}

fn conix_disclosure(m: &Mounted, d: &mut port::PluginDisclosure) {
    for name in m.flow_names() {
        d.conix.push(format!("flow `{name}`"));
    }
    for (gate, t) in &m.section.gates {
        let how = if t.runs_commands() {
            "may run commands that change things, as the project's permission rules allow"
        } else {
            "read-only call"
        };
        d.conix.push(format!(
            "host gate `{gate}` → plugin function `{}` ({how}{})",
            t.function(),
            longer(t.timeout_ms())
        ));
    }
    for (step, t) in &m.section.steps {
        d.conix.push(format!(
            "program step `{step}` → plugin function `{}` (may write{})",
            t.function(),
            longer(t.timeout_ms())
        ));
    }
    match m.section.governed.flows() {
        Some(None) => d.conix.push(
            "governs all its flows: a project cannot replace them or their parts, and their runs cannot be forced".into(),
        ),
        Some(Some(names)) => d.conix.push(format!(
            "governs flows {}: a project cannot replace them or the plugin's parts, and their runs cannot be forced",
            names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(", ")
        )),
        None => {}
    }
    // 流程包里自带的命令门禁会在这台机器上执行命令，脚本门禁会读运行的工作目录：都写出来。
    if let Some(dir) = m.flows_dir() {
        let mut gates: Vec<_> = std::fs::read_dir(dir.join("gates"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
            .collect();
        gates.sort();
        for p in gates {
            let Ok(g) = std::fs::read_to_string(&p)
                .map_err(|e| e.to_string())
                .and_then(|t| {
                    serde_yaml::from_str::<harnix_flow::GateDef>(&t).map_err(|e| e.to_string())
                })
            else {
                continue;
            };
            match g.kind {
                harnix_flow::GateKind::Command => d.conix.push(format!(
                    "command gate `{}` runs {} on this machine (in trusted projects){}",
                    g.name,
                    // `plan` 的门禁没有命令行：跑什么由包里的脚本从人声明的清单里挑。
                    match (&g.plan, &g.command) {
                        (Some(s), _) => format!("what its script `{s}` picks"),
                        (None, c) => format!("`{}`", c.as_deref().unwrap_or_default()),
                    },
                    if g.env.is_empty() {
                        String::new()
                    } else {
                        format!(", given {}", g.env.join(", "))
                    }
                )),
                harnix_flow::GateKind::Script if g.script.is_some() => d.conix.push(format!(
                    "script gate `{}` runs scripts/{}.js, reading the run's workspace",
                    g.name,
                    g.script.as_deref().unwrap_or_default()
                )),
                _ => {}
            }
        }
    }
    if m.section.flows.is_some() && m.flows_dir().is_none() {
        d.conix
            .push("its flows directory is missing or outside the package: not mounted".into());
    }
    // 流程包里的 skill 与规则在派活时进站的消息，一字不差到达模型。
    if let Some(dir) = m.flows_dir() {
        for (kind, sub) in [("skill", "skills"), ("rule", "rules")] {
            for e in std::fs::read_dir(dir.join(sub))
                .into_iter()
                .flatten()
                .flatten()
            {
                let p = e.path();
                let (name, file) = if p.is_dir() {
                    (
                        p.file_name().and_then(|f| f.to_str()).map(str::to_string),
                        p.join("SKILL.md"),
                    )
                } else {
                    (
                        p.file_stem().and_then(|f| f.to_str()).map(str::to_string),
                        p,
                    )
                };
                let (Some(name), Ok(text)) = (name, std::fs::read_to_string(&file)) else {
                    continue;
                };
                d.conix_model_visible.push(port::VisibleText {
                    origin: format!("flow pack {kind} `{name}`"),
                    text,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_names_split_at_the_version() {
        assert_eq!(
            split_package_name("xforge-0.1.0.zip"),
            Some(("xforge".into(), "0.1.0".into()))
        );
        assert_eq!(
            split_package_name("code-review-helper-1.2.0-rc1.zip"),
            Some(("code-review-helper".into(), "1.2.0-rc1".into()))
        );
        assert_eq!(split_package_name("notes.zip"), None);
        assert_eq!(split_package_name("x-1.0.tar"), None);
    }

    #[test]
    fn the_conix_section_is_read_and_flows_stay_in_the_package() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("plugin.toml"),
            r#"uses = ["conix"]
[plugin]
name = "xf"
version = "0.1.0"
api_version = "0.1"

[x-conix]
flows = "flows"
[x-conix.gates]
xf-structure = "xf.check"
[x-conix.steps]
xf-open = "xf.open"
"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("flows/flows")).unwrap();
        std::fs::write(
            dir.path().join("flows/flows/xf-quick.yaml"),
            "name: xf-quick\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("flows/skills/xf-propose")).unwrap();
        std::fs::write(
            dir.path().join("flows/skills/xf-propose/SKILL.md"),
            "propose it",
        )
        .unwrap();
        let section = conix_section(dir.path()).unwrap();
        assert_eq!(section.gates["xf-structure"].function(), "xf.check");
        let m = Mounted {
            name: "xf".into(),
            root: dir.path().to_path_buf(),
            section,
        };
        assert_eq!(m.flow_names(), ["xf-quick"]);
        let d = disclosure(&serde_json::json!({"capabilities": ["x"]}), Some(&m));
        assert_eq!(d.capabilities, ["x"]);
        assert!(
            d.conix
                .iter()
                .any(|l| l.contains("xf-open") && l.contains("xf.open"))
        );
        assert_eq!(d.conix_model_visible[0].text, "propose it");

        let out = Mounted {
            section: ConixSection {
                flows: Some("../elsewhere".into()),
                ..Default::default()
            },
            ..m.clone()
        };
        assert_eq!(out.flows_dir(), None, "包外的不挂");
    }

    #[test]
    fn the_first_plugin_by_name_keeps_a_gate_name() {
        let p = |name: &str| Mounted {
            name: name.into(),
            root: PathBuf::new(),
            section: ConixSection {
                gates: BTreeMap::from([(
                    "g".to_string(),
                    GateTarget::Full {
                        function: format!("{name}.check"),
                        runs_commands: name == "zeta",
                        timeout: (name == "zeta").then(|| "8m".into()),
                    },
                )]),
                ..Default::default()
            },
        };
        let m = PluginMap::build(&[p("zeta"), p("alpha")]);
        assert_eq!(m.gates["g"], ("alpha".into(), "alpha.check".into()));
        assert!(m.runs_commands.is_empty(), "胜出的那个没要跑命令");
        assert!(m.gate_timeouts.is_empty(), "时限也只算胜出的那个");
        assert_eq!(m.conflicts["zeta"].len(), 1);
    }

    /// 门禁与步骤可以写时限：缺省两分钟，至多十分钟（Agix `plugin_call_long`），写错的用缺省；披露里写明长的。
    #[test]
    fn gates_and_steps_may_ask_for_up_to_ten_minutes() {
        let section: ConixSection = toml::from_str::<Manifest>(
            r#"
[x-conix.gates]
quick = "p.quick"
tests = { function = "p.tests", runs_commands = true, timeout = "8m" }
huge = { function = "p.huge", timeout = "2h" }
typo = { function = "p.typo", timeout = "eight minutes" }

[x-conix.steps]
pack = { function = "p.pack", timeout = "3m30s" }
plain = "p.plain"
"#,
        )
        .unwrap()
        .x_conix
        .unwrap();
        let m = Mounted {
            name: "p".into(),
            root: PathBuf::new(),
            section,
        };
        let map = PluginMap::build(std::slice::from_ref(&m));
        assert_eq!(
            map.gate_timeouts,
            BTreeMap::from([("tests".into(), 480_000), ("huge".into(), 600_000)])
        );
        assert_eq!(
            map.step_timeouts,
            BTreeMap::from([("pack".into(), 210_000)])
        );
        assert_eq!(map.steps["plain"], ("p".into(), "p.plain".into()));
        let mut d = port::PluginDisclosure::default();
        conix_disclosure(&m, &mut d);
        assert!(
            d.conix
                .iter()
                .any(|l| l.contains("`tests`") && l.ends_with("up to 8 min)")),
            "{:?}",
            d.conix
        );
        assert!(
            d.conix
                .iter()
                .any(|l| l.contains("`pack`") && l.ends_with("up to 3 min 30 s)")),
            "{:?}",
            d.conix
        );
        assert!(
            d.conix
                .iter()
                .any(|l| l.contains("`quick`") && l.ends_with("(read-only call)"))
        );
    }
}
