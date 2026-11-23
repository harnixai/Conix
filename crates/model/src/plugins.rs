//! `/plugin`（plugins.md §3）：列表浮层与两份披露浮层的状态变化和画面。

use keys::Action;
use port::{Command, PluginDisclosure, PluginInfo, Plugins};
use tui::{Line, Tone};

use crate::state::{Overlay, State};
use crate::update::{Effect, Pending};

/// 列表里的一行：装好的在前，没装的包在后。
pub(crate) enum Row<'a> {
    Installed(&'a PluginInfo),
    Available(&'a port::PluginPackage),
}

pub(crate) fn rows(p: &Plugins) -> Vec<Row<'_>> {
    p.installed
        .iter()
        .map(Row::Installed)
        .chain(
            p.available
                .iter()
                .filter(|a| !a.installed && !p.installed.iter().any(|i| i.name == a.name))
                .map(Row::Available),
        )
        .collect()
}

/// `/plugin` 的参数：空 = 列表浮层，其余是子命令。
pub(crate) fn command(s: &mut State, arg: &str) -> Vec<Effect> {
    let (sub, rest) = arg.split_once(' ').unwrap_or((arg, ""));
    let rest = rest.trim().to_string();
    if sub.is_empty() {
        return vec![s.call(Pending::Plugins, Command::ListPlugins)];
    }
    if rest.is_empty() {
        s.notice(i18n::t!("plugins.usage"), true);
        return vec![];
    }
    vec![match sub {
        "install" => s.call(
            Pending::PluginInstalled,
            Command::InstallPlugin { source: rest },
        ),
        "enable" | "disable" => s.call(
            Pending::PluginSet(rest.clone(), sub == "enable"),
            Command::SetPluginEnabled {
                name: rest,
                enabled: sub == "enable",
            },
        ),
        "remove" => s.call(
            Pending::PluginRemoved(rest.clone()),
            Command::UninstallPlugin { name: rest },
        ),
        "info" => s.call(
            Pending::PluginInfo,
            Command::PluginDisclosure { name: rest },
        ),
        _ => {
            s.notice(i18n::t!("plugins.usage"), true);
            return vec![];
        }
    }]
}

pub(crate) fn listed(s: &mut State, plugins: Plugins) {
    let n = rows(&plugins).len();
    match &mut s.ui.overlay {
        // 事件触发的重列：留在原来的位置。
        Some(Overlay::Plugins {
            plugins: old,
            selected,
            armed,
        }) => {
            *old = plugins;
            *selected = (*selected).min(n.saturating_sub(1));
            *armed = None;
        }
        _ => {
            s.ui.overlay = Some(Overlay::Plugins {
                plugins,
                selected: 0,
                armed: None,
            })
        }
    }
}

pub(crate) fn disclosed(
    s: &mut State,
    plugin: PluginInfo,
    disclosure: PluginDisclosure,
    installing: bool,
) {
    s.ui.overlay = Some(Overlay::PluginDisclosure {
        plugin: Box::new(plugin),
        disclosure: Box::new(disclosure),
        scroll: 0,
        installing,
    });
}

fn set(s: &mut State, name: String, enabled: bool) -> Effect {
    s.call(
        Pending::PluginSet(name.clone(), enabled),
        Command::SetPluginEnabled { name, enabled },
    )
}

fn remove(s: &mut State, name: String) -> Effect {
    s.call(
        Pending::PluginRemoved(name.clone()),
        Command::UninstallPlugin { name },
    )
}

pub(crate) fn list_action(s: &mut State, a: Action) -> Vec<Effect> {
    let Some(Overlay::Plugins {
        plugins,
        selected,
        armed,
    }) = &mut s.ui.overlay
    else {
        return vec![];
    };
    let was_armed = armed.take();
    let n = rows(plugins).len();
    // 先把选中那行拷出来：后面要改 overlay。
    enum Pick {
        Install(String),
        Info(String),
        Toggle(String, bool),
        Delete(String),
    }
    let pick = match (a, rows(plugins).get(*selected)) {
        (Action::Up, _) => {
            *selected = selected.saturating_sub(1);
            None
        }
        (Action::Down, _) => {
            *selected = (*selected + 1).min(n.saturating_sub(1));
            None
        }
        (Action::Confirm | Action::Open | Action::Submit, Some(Row::Available(p))) => {
            Some(Pick::Install(p.path.clone()))
        }
        (Action::Confirm | Action::Open | Action::Submit, Some(Row::Installed(i))) => {
            Some(Pick::Info(i.name.clone()))
        }
        (Action::ToggleEnabled, Some(Row::Installed(i))) => {
            Some(Pick::Toggle(i.name.clone(), !i.enabled))
        }
        (Action::Delete, Some(Row::Installed(i))) => Some(Pick::Delete(i.name.clone())),
        (Action::Back | Action::No, _) => {
            s.ui.overlay = None;
            None
        }
        _ => None,
    };
    match pick {
        Some(Pick::Install(path)) => vec![s.call(
            Pending::PluginInstalled,
            Command::InstallPlugin { source: path },
        )],
        Some(Pick::Info(name)) => {
            vec![s.call(Pending::PluginInfo, Command::PluginDisclosure { name })]
        }
        // 浮层留着：PluginsChanged 会把它重列。
        Some(Pick::Toggle(name, enabled)) => vec![set(s, name, enabled)],
        Some(Pick::Delete(name)) => {
            if was_armed.as_deref() == Some(name.as_str()) {
                vec![remove(s, name)]
            } else if let Some(Overlay::Plugins { armed, .. }) = &mut s.ui.overlay {
                *armed = Some(name);
                vec![]
            } else {
                vec![]
            }
        }
        None => vec![],
    }
}

pub(crate) fn disclosure_action(s: &mut State, a: Action) -> Vec<Effect> {
    let Some(Overlay::PluginDisclosure {
        plugin,
        scroll,
        installing,
        ..
    }) = &mut s.ui.overlay
    else {
        return vec![];
    };
    match a {
        Action::Up | Action::LineUp => *scroll = scroll.saturating_sub(1),
        Action::Down | Action::LineDown => *scroll += 1,
        Action::PageUp => *scroll = scroll.saturating_sub(10),
        Action::PageDown => *scroll += 10,
        Action::Confirm | Action::Submit | Action::Yes if *installing => {
            let name = plugin.name.clone();
            s.ui.overlay = None;
            return vec![set(s, name, true)];
        }
        // 不确认就不装：撤回刚装下的。
        Action::Back | Action::No if *installing => {
            let name = plugin.name.clone();
            s.ui.overlay = None;
            return vec![remove(s, name)];
        }
        Action::Back | Action::No | Action::Confirm | Action::Submit | Action::Yes => {
            s.ui.overlay = None;
        }
        _ => {}
    }
    vec![]
}

pub(crate) fn list_view(
    s: &State,
    plugins: &Plugins,
    selected: usize,
    armed: &Option<String>,
) -> tui::Overlay {
    let mut lines: Vec<Line> = vec![];
    for (i, r) in rows(plugins).iter().enumerate() {
        let mark = if i == selected { "› " } else { "  " };
        let mut l = Line::plain(mark);
        match r {
            Row::Installed(p) => {
                let (g, tone) = if p.enabled {
                    ("✓", Tone::Ok)
                } else {
                    ("○", Tone::Dim)
                };
                l = l
                    .push(format!("{g} "), tone)
                    .push(p.name.clone(), Tone::Normal)
                    .push(
                        format!(
                            "  {}  {}",
                            p.version,
                            i18n::t!("plugins.n_flows", n = p.flows.len())
                        ),
                        Tone::Dim,
                    );
                if let Some(why) = &p.disabled_reason {
                    l = l.push(format!("  {why}"), Tone::Warn);
                }
                for c in &p.conflicts {
                    l = l.push(format!("  {c}"), Tone::Dim);
                }
            }
            Row::Available(p) => {
                l = l
                    .push("+ ", Tone::Accent)
                    .push(format!("{} {}", p.name, p.version), Tone::Normal)
                    .push(format!("  {}", p.path), Tone::Dim);
            }
        }
        if i == selected {
            l = l.with_tone(Tone::Selected);
        }
        lines.push(l);
    }
    if lines.is_empty() {
        lines.push(Line::toned(i18n::t!("plugins.empty"), Tone::Dim));
    }
    let body = s.ui.size.rows.saturating_sub(8) as usize;
    let skip = selected.saturating_sub(body.saturating_sub(1));
    lines = lines.into_iter().skip(skip).take(body).collect();
    lines.push(Line::default());
    lines.push(match armed {
        Some(name) => Line::toned(i18n::t!("plugins.armed", name = name), Tone::Warn),
        None => Line::toned(i18n::t!("plugins.keys"), Tone::Dim),
    });
    tui::Overlay {
        title: i18n::t!("plugins.title"),
        height: lines.len() as u16 + 2,
        lines,
        width: (s.ui.size.cols.saturating_sub(10)).min(110),
    }
}

pub(crate) fn disclosure_view(
    s: &State,
    plugin: &PluginInfo,
    d: &PluginDisclosure,
    scroll: usize,
    installing: bool,
) -> tui::Overlay {
    let width = (s.ui.size.cols.saturating_sub(10)).min(110);
    let inner = (width as usize).saturating_sub(4).max(20);
    let mut all: Vec<Line> = vec![];
    let section = |all: &mut Vec<Line>, title: String| {
        if !all.is_empty() {
            all.push(Line::default());
        }
        all.push(Line::toned(title, Tone::Strong));
    };
    let bullets = |all: &mut Vec<Line>, items: &[String]| {
        if items.is_empty() {
            all.push(Line::toned(
                format!("  {}", i18n::t!("plugins.none")),
                Tone::Dim,
            ));
        }
        for t in items {
            all.extend(crate::text::wrap_text(
                t,
                Tone::Normal,
                inner,
                &Line::plain("  · "),
                &Line::plain("    "),
            ));
        }
    };
    let visible = |all: &mut Vec<Line>, items: &[port::VisibleText]| {
        if items.is_empty() {
            all.push(Line::toned(
                format!("  {}", i18n::t!("plugins.none")),
                Tone::Dim,
            ));
        }
        for v in items {
            all.push(Line::toned(format!("  {}", v.origin), Tone::Accent));
            all.extend(crate::text::wrap_text(
                &v.text,
                Tone::Normal,
                inner,
                &Line::plain("    "),
                &Line::plain("    "),
            ));
        }
    };
    let desc = if plugin.description.is_empty() {
        String::new()
    } else {
        format!(" — {}", plugin.description)
    };
    all.push(Line::toned(
        format!("{} {}{desc}", plugin.name, plugin.version),
        Tone::Strong,
    ));
    section(&mut all, i18n::t!("plugins.agix"));
    all.push(Line::toned(
        format!(" {}", i18n::t!("plugins.capabilities")),
        Tone::Dim,
    ));
    bullets(&mut all, &d.capabilities);
    all.push(Line::toned(
        format!(" {}", i18n::t!("plugins.mcp")),
        Tone::Dim,
    ));
    bullets(&mut all, &d.mcp_servers);
    all.push(Line::toned(
        format!(" {}", i18n::t!("plugins.model_visible")),
        Tone::Dim,
    ));
    visible(&mut all, &d.model_visible);
    section(&mut all, i18n::t!("plugins.codix"));
    bullets(&mut all, &d.codix);
    all.push(Line::toned(
        format!(" {}", i18n::t!("plugins.model_visible")),
        Tone::Dim,
    ));
    visible(&mut all, &d.codix_model_visible);

    let height = (all.len() as u16 + 4)
        .min(s.ui.size.rows.saturating_sub(4))
        .max(7);
    let body = height.saturating_sub(4) as usize;
    let max = all.len().saturating_sub(body);
    let mut lines: Vec<Line> = all.into_iter().skip(scroll.min(max)).take(body).collect();
    lines.push(Line::default());
    lines.push(Line::toned(
        if installing {
            i18n::t!("plugins.keys_installing")
        } else {
            i18n::t!("plugins.keys_info")
        },
        Tone::Dim,
    ));
    tui::Overlay {
        title: i18n::t!("plugins.disclosure_title", name = plugin.name),
        height: lines.len() as u16 + 2,
        lines,
        width,
    }
}
