//! `/plugin` 的端口命令（docs/design/plugins.md §3）：装、卸、启停交给 Agix（`plugin.*`），Codix 读包里的
//! `[x-codix]`，把启用着的插件的流程包挂进流程库、门禁与步骤接到插件函数上。

use std::path::{Path, PathBuf};

use super::*;
use crate::plugins::{self, Mounted};

impl Hub {
    pub fn set_plugin_dirs(&self, dirs: Vec<PathBuf>) {
        *self.0.plugin_dirs.locked() = dirs;
    }

    async fn plugin_list(&self) -> Result<Vec<Value>, PortError> {
        let r = self.call("plugin.list", json!({})).await.map_err(rpc_err)?;
        Ok(r["plugins"].as_array().cloned().unwrap_or_default())
    }

    /// 启用着、带 `[x-codix]` 的插件。问不到（插件关着、旧引擎）就当没有：流程照常，只是没有插件的。
    pub(crate) async fn mounted_plugins(&self) -> Vec<Mounted> {
        let list = match self.plugin_list().await {
            Ok(l) => l,
            Err(e) => {
                tracing::debug!("plugin.list: {e}");
                return vec![];
            }
        };
        list.iter()
            .filter(|p| p["enabled"] == true)
            .filter_map(mounted_of)
            .collect()
    }

    /// 插件变了之后：流程包跟着换，`/plugin` 浮层重新列。
    async fn plugins_refresh(&self) {
        let mounted = self.mounted_plugins().await;
        if let Some(f) = self.flows() {
            f.plugins_changed(mounted).await;
        }
        self.emit_events(vec![Event::PluginsChanged]);
    }

    pub(super) async fn plugin_command(&self, cmd: Command) -> Result<Reply, PortError> {
        match cmd {
            Command::ListPlugins => Ok(Reply::Plugins(self.plugins().await?)),
            Command::InstallPlugin { source } => self.install_plugin(&source).await,
            Command::SetPluginEnabled { name, enabled } => {
                let method = if enabled {
                    "plugin.enable"
                } else {
                    "plugin.disable"
                };
                self.call(method, json!({"name": name, "scope": "global"}))
                    .await
                    .map_err(rpc_err)?;
                self.plugins_refresh().await;
                Ok(Reply::Ok)
            }
            Command::UninstallPlugin { name } => {
                self.call("plugin.uninstall", json!({"name": name, "scope": "global"}))
                    .await
                    .map_err(rpc_err)?;
                self.plugins_refresh().await;
                Ok(Reply::Ok)
            }
            Command::PluginDisclosure { name } => {
                let list = self.plugin_list().await?;
                let p = list
                    .iter()
                    .find(|p| p["name"] == name.as_str())
                    .ok_or_else(|| PortError::Failed(format!("no plugin `{name}` is installed")))?;
                let m = mounted_of(p);
                Ok(Reply::Plugin {
                    plugin: self.info_of(p, m.as_ref()),
                    disclosure: plugins::disclosure(&listed_disclosure(p), m.as_ref()),
                })
            }
            _ => Err(PortError::Failed("not a plugin command".into())),
        }
    }

    async fn plugins(&self) -> Result<port::Plugins, PortError> {
        let list = self.plugin_list().await?;
        let installed: Vec<port::PluginInfo> = list
            .iter()
            .map(|p| self.info_of(p, mounted_of(p).as_ref()))
            .collect();
        let dirs = self.0.plugin_dirs.locked().clone();
        let available = plugins::packages(&dirs)
            .into_iter()
            .map(|mut p| {
                p.installed = installed.iter().any(|i| i.name == p.name);
                p
            })
            .collect();
        Ok(port::Plugins {
            available,
            installed,
        })
    }

    fn info_of(&self, p: &Value, m: Option<&Mounted>) -> port::PluginInfo {
        let name = p["name"].as_str().unwrap_or_default().to_string();
        let mut conflicts: Vec<String> = self
            .flows()
            .map(|f| f.plugin_conflicts().remove(&name).unwrap_or_default())
            .unwrap_or_default();
        for c in p["command_conflicts"].as_array().into_iter().flatten() {
            conflicts.push(format!(
                "command `{}` is taken by {} {}",
                c["command"].as_str().unwrap_or_default(),
                c["shadowed_by"].as_str().unwrap_or_default(),
                c["plugin"]
                    .as_str()
                    .or(c["path"].as_str())
                    .unwrap_or_default()
            ));
        }
        for c in p["skill_conflicts"].as_array().into_iter().flatten() {
            conflicts.push(format!(
                "skill `{}` is taken by {}",
                c["skill"].as_str().unwrap_or_default(),
                c["shadowed_by"].as_str().unwrap_or_default()
            ));
        }
        port::PluginInfo {
            name,
            version: p["version"].as_str().unwrap_or_default().to_string(),
            description: p["description"].as_str().unwrap_or_default().to_string(),
            enabled: p["enabled"] == true,
            disabled_reason: p["disabled_reason"].as_str().map(str::to_string),
            flows: m.map(Mounted::flow_names).unwrap_or_default(),
            gates: m
                .map(|m| m.section.gates.keys().cloned().collect())
                .unwrap_or_default(),
            steps: m
                .map(|m| m.section.steps.keys().cloned().collect())
                .unwrap_or_default(),
            conflicts,
        }
    }

    /// 装一个包：名字（在可装的包里找，同名取版本排最后的）或 zip 的路径。装完是停用的（Agix 0.4 起
    /// `plugins.enable_on_install` 缺省 false），人看过两份披露确认后才启用。
    async fn install_plugin(&self, source: &str) -> Result<Reply, PortError> {
        let dirs = self.0.plugin_dirs.locked().clone();
        let zip: PathBuf = if source.ends_with(".zip") || source.contains('/') {
            PathBuf::from(source)
        } else {
            plugins::packages(&dirs)
                .into_iter()
                .rfind(|p| p.name == source)
                .map(|p| PathBuf::from(p.path))
                .ok_or_else(|| {
                    PortError::Failed(format!("no package named `{source}` in {dirs:?}"))
                })?
        };
        let zip = zip
            .canonicalize()
            .map_err(|e| PortError::Failed(format!("{}: {e}", zip.display())))?;
        let file = zip.file_name().and_then(|f| f.to_str()).unwrap_or_default();
        let (name, version) = plugins::split_package_name(file).ok_or_else(|| {
            PortError::Failed(format!("{file}: a package is named <name>-<version>.zip"))
        })?;
        let checksum = plugins::sha256_file(&zip)
            .map_err(|e| PortError::Failed(format!("{}: {e}", zip.display())))?;
        let r = self
            .call(
                "plugin.install",
                json!({"name": name, "version": version, "checksum": checksum, "scope": "global",
                       "download_url": format!("file://{}", zip.display())}),
            )
            .await
            .map_err(rpc_err)?;
        let list = self.plugin_list().await?;
        let p = list
            .iter()
            .find(|p| p["name"] == name.as_str())
            .cloned()
            .unwrap_or_else(|| json!({"name": name, "version": version}));
        let m = mounted_of(&p);
        let reply = Reply::Plugin {
            plugin: self.info_of(&p, m.as_ref()),
            disclosure: plugins::disclosure(&r["disclosure"], m.as_ref()),
        };
        self.emit_events(vec![Event::PluginsChanged]);
        Ok(reply)
    }
}

/// `plugin.list` 的一项 → 带 `[x-codix]` 的就是 Codix 要挂的（启用与否由调用方看）。
fn mounted_of(p: &Value) -> Option<Mounted> {
    let root = PathBuf::from(p["root"].as_str()?);
    let section = plugins::codix_section(Path::new(&root))?;
    Some(Mounted {
        name: p["name"].as_str()?.to_string(),
        root,
        section,
    })
}

/// 再看一次披露：Agix 没有"再给一次 disclosure"的方法，按 `plugin.list` 报的命令与函数拼出能力那几条
/// （措辞同 Agix 的），到达模型的只有插件描述与命令描述能从这里拿到。
fn listed_disclosure(p: &Value) -> Value {
    let mut caps = vec![];
    let mut visible = vec![];
    if let Some(d) = p["description"].as_str().filter(|d| !d.is_empty()) {
        visible.push(json!({"origin": "plugin description", "text": d}));
    }
    let writes = |w: &Value| {
        let w: Vec<&str> = w
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        if w.is_empty() {
            "writes no files".to_string()
        } else {
            format!("may write {}", w.join(", "))
        }
    };
    for c in p["commands"].as_array().into_iter().flatten() {
        let name = c["name"].as_str().unwrap_or_default();
        caps.push(format!(
            "adds command `{name}` that {} through the file tools",
            writes(&c["writes"])
        ));
        if let Some(d) = c["description"].as_str() {
            visible.push(json!({"origin": format!("command `{name}` description"), "text": d}));
        }
    }
    let plugin = p["name"].as_str().unwrap_or_default();
    for f in p["functions"].as_array().into_iter().flatten() {
        caps.push(format!(
            "exposes function `{plugin}.{}` to the host; {}",
            f["name"].as_str().unwrap_or_default(),
            writes(&f["writes"])
        ));
    }
    for s in p["skills"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        caps.push(format!("adds skill `{s}`"));
    }
    json!({"capabilities": caps, "model_visible": visible})
}
