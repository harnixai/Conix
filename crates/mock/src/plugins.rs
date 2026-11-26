//! 插件的脚本化行为（plugins.md §3）。包里的内容在真实后端是读 zip 得来的，mock 没有 zip，
//! 所以只认一个写死的 `xforge`：别的名字当作"没有这个包"，让 TUI 的出错路径也走得到。

use port::{
    Event, FlowOrigin, FlowSummary, PluginDisclosure, PluginInfo, PluginPackage, Plugins,
    PortError, VisibleText,
};

use crate::seed::FlowSeed;
use crate::state::MockState;

const FLOWS: [(&str, &str); 2] = [("xforge-quick", "快速变更"), ("xforge-solid", "常规变更")];

fn failed(msg: String) -> PortError {
    PortError::Failed(msg)
}

fn info_of(pkg: &PluginPackage) -> PluginInfo {
    PluginInfo {
        name: pkg.name.clone(),
        version: pkg.version.clone(),
        description: "按规格推进变更的一套流程与门禁".into(),
        enabled: false,
        disabled_reason: None,
        flows: FLOWS.iter().map(|(n, _)| n.to_string()).collect(),
        gates: vec!["xforge-structure".into()],
        steps: vec!["xforge-open".into()],
        conflicts: vec![],
    }
}

fn disclosure_of(name: &str) -> PluginDisclosure {
    let v = |origin: &str, text: &str| VisibleText {
        origin: origin.into(),
        text: text.into(),
    };
    PluginDisclosure {
        capabilities: vec![
            "adds command `xflow` that may write xforge/** through the file tools; it runs in the session that invokes it"
                .into(),
            format!("exposes function `{name}.check` to the host; writes no files"),
            "uses host library `conix` (Conix 的流程与作业)".into(),
            "run its own JavaScript at the `tool.around` extension point".into(),
        ],
        mcp_servers: vec![],
        model_visible: vec![
            v("plugin.description", "按规格推进变更的一套流程与门禁。"),
            v(
                "agent.prompt",
                "你是 xforge 的变更助手：先读 xforge/spec.md，再按站推进；每一站结束前跑 xforge.check……",
            ),
            v("command xflow", "按 xforge 流程推进一项变更"),
        ],
        conix: vec![
            "流程 xforge-quick — 快速变更".into(),
            "流程 xforge-solid — 常规变更".into(),
            "门禁 xforge-structure → xforge.check".into(),
            "步骤 xforge-open → xforge.open".into(),
        ],
        conix_model_visible: vec![v(
            "xforge-solid / 站 design / skill",
            "先列出受影响的模块与接口，再写设计；设计必须说明为什么不选更简单的做法……",
        )],
    }
}

pub fn list(st: &MockState) -> Plugins {
    Plugins {
        available: st
            .plugin_packages
            .iter()
            .map(|p| PluginPackage {
                installed: st.plugins.iter().any(|i| i.name == p.name),
                ..p.clone()
            })
            .collect(),
        installed: st.plugins.clone(),
    }
}

pub fn install(
    st: &mut MockState,
    source: &str,
) -> Result<(PluginInfo, PluginDisclosure), PortError> {
    let pkg = st
        .plugin_packages
        .iter()
        .find(|p| p.name == source || p.path == source || source.ends_with(&file_name(p)))
        .cloned()
        .ok_or_else(|| failed(format!("没有这个插件包：{source}")))?;
    if st.plugins.iter().any(|i| i.name == pkg.name) {
        return Err(failed(format!("插件 {} 已经装了", pkg.name)));
    }
    let info = info_of(&pkg);
    st.plugins.push(info.clone());
    Ok((info, disclosure_of(&pkg.name)))
}

fn file_name(p: &PluginPackage) -> String {
    format!("{}-{}.zip", p.name, p.version)
}

pub fn disclosure(st: &MockState, name: &str) -> Result<(PluginInfo, PluginDisclosure), PortError> {
    let info = st
        .plugins
        .iter()
        .find(|i| i.name == name)
        .ok_or_else(|| failed(format!("没有装插件 {name}")))?;
    Ok((info.clone(), disclosure_of(name)))
}

fn flow_summaries(info: &PluginInfo) -> Vec<FlowSummary> {
    FLOWS
        .iter()
        .filter(|(n, _)| info.flows.iter().any(|f| f == n))
        .map(|(n, t)| FlowSummary {
            name: (*n).into(),
            title: (*t).into(),
            origin: FlowOrigin::Plugin,
            active_runs: 0,
            source: Some(info.name.clone()),
            bundle: None,
        })
        .collect()
}

/// 状态在这里直接改好；事件只负责通知，所以调用方用不会再生效一次的 `emit`。
pub fn set_enabled(st: &mut MockState, name: &str, enabled: bool) -> Result<Vec<Event>, PortError> {
    let info = st
        .plugins
        .iter_mut()
        .find(|i| i.name == name)
        .ok_or_else(|| failed(format!("没有装插件 {name}")))?;
    if info.enabled == enabled {
        return Ok(vec![]);
    }
    info.enabled = enabled;
    let info = info.clone();
    let mut events = vec![];
    for f in flow_summaries(&info) {
        if enabled {
            st.flows.retain(|x| x.summary.name != f.name);
            st.flows.push(FlowSeed {
                summary: f.clone(),
                stations: vec![],
                yaml: String::new(),
            });
            events.push(Event::FlowUpserted(f));
        } else {
            st.flows.retain(|x| x.summary.name != f.name);
            events.push(Event::FlowRemoved { name: f.name });
        }
    }
    events.push(Event::PluginsChanged);
    Ok(events)
}

pub fn uninstall(st: &mut MockState, name: &str) -> Result<Vec<Event>, PortError> {
    let mut events = if st.plugins.iter().any(|i| i.name == name && i.enabled) {
        set_enabled(st, name, false)?
    } else {
        vec![]
    };
    let before = st.plugins.len();
    st.plugins.retain(|i| i.name != name);
    if st.plugins.len() == before {
        return Err(failed(format!("没有装插件 {name}")));
    }
    if !events.iter().any(|e| matches!(e, Event::PluginsChanged)) {
        events.push(Event::PluginsChanged);
    }
    Ok(events)
}
