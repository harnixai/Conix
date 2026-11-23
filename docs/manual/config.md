# 配置与文件

## 文件在哪

```
~/.codix/                       全局（CODIX_HOME 可改）；也是 Agix 的数据根
  settings.json                 Agix 的设置：模型、provider、权限……（见「Agix 的设置」）
  codix.yaml                    Codix 自己的配置（下面）
  keybindings.json              键位覆盖（下面）
  flows/                        全局流程包
  instances/<名字>/             实例的运行时文件：socket、锁、发现文件、日志
<项目>/
  .wkts/<运行>/                 流程运行的 worktree（不进 git）
  .codix/
    settings.json  agents/ …    项目的引擎配置（进 git）
    flows/                      项目自己的流程（进 git）
    jobs.json                   作业定义（进 git）
    settings.local.json …       个人配置（不进 git）
    state/                      本机数据与过程数据（不进 git）
      history/                  受信项目的会话
      codix/                    会话归属、作业状态与触发记录、流程运行记录与过程目录、每日用量、通知记录
```

`.codix/.gitignore` 由 Agix 维护（`state/`、`settings.local.json`、`mcp.local.json`）。流程运行的过程目录在
`state/codix/runs/` 下，跟着 `state/` 不进 git；以前版本留在 `.codix/runs/` 的不再读，可以删掉。
`state/codix/` 只认这台机器自己写的：从仓库里带来的会被挪进 `state/codix/quarantine/`，不读。

## codix.yaml

不存在时会写一份带注释的缺省配置。读不懂的项用缺省，原因在 `/doctor` 与日志里。

```yaml
ui:
  language: zh              # zh | en | auto（照 LANG）；CODIX_LANG 可临时指定
  initial_size: 160x45      # 启动时请求的终端尺寸；off 不请求
  ratio: 60                 # 宽屏时左栏占的百分比，40–75
  compact_tool_lines: 3     # 精简模式下工具结果显示几行
  syntax_theme: base16-ocean.dark
  theme:
    preset: claude          # claude | aqua
    colors: {}              # 逐项覆盖：accent border ok warn error …

instance:
  idle_timeout: 30m         # 空闲多久自动脱离；0 不超时

jobs:
  defaults:                 # 作业没写时用
    timeout: 30m
    max_tokens: 2M          # 一次触发的 token：未命中缓存的输入 + 缓存写入 + 输出（缓存命中不计）
    overlap: skip           # skip | queue | replace
    disable_after: 3        # 0 不自动停用
    network: deny_all       # 作业里命令的出网：deny_all 不出网 | allowlist 只到 web.allowed_domains | inherit 照设置
  limits:                   # 天花板：作业定义写得再大也不超过它
    timeout: 4h
    max_tokens: 20M
  daily_tokens: 50M         # 所有无人值守的工作一天合计；0 不限
  max_concurrent: 2

notify: …                   # 见「通知」一章
```

## Agix 的设置

模型、provider、权限、沙箱这些是 Agix 的 `settings.json`：全局 `~/.codix/settings.json`，项目 `<项目>/.codix/settings.json`
（个人的放 `settings.local.json`）。键与含义见 [Agix 设置](../../agix/docs/settings.md)，这里不重复。

- **0.4 不再读旧键**（`model.model_name`、`execution.*`、`permission_rules` 之类）：读出来是空，不报错。升级后跑一遍 `/doctor`，
  Agix 的 `settings.tiers` 一项会列出 `旧 → 新`，被丢掉的值也在里面。迁移说明见 [升级到 0.4](../../agix/docs/upgrading-0.4.md)。
- **模型**：`providers` 是列表，缺省模型取 `model.order` 第一个 provider 的 balanced 档（没写取 strong 档）。没定义 `providers` 时用
  Agix 自带的 `anthropic`，它的模型不在设置里，用环境变量 `CODIX_MODEL` 指定（不设就是 Agix 的缺省）。要按项目或档位选模型，就定义 `providers`。
- **Codix 在 cli 层写的**（优先级低于你的设置，只补缺省）：`permissions.allow` 放行只读工具与流程自己的工具，免得每次都问；
  `permissions.client_may_loosen: true`，让流程的站能用 `acceptEdits`（0.4 的会话选项只能收紧，不开这个就被夹回，无人看着的运行会停在权限提问上）；
  `app.history: "project"`，受信项目的会话放进项目里。这个开关是 Agix 说的"安全缺省"而不是安全边界，部署方的 managed 设置仍然压过它，
  能把它关掉；关了 `/doctor` 的 `codix.session_loosen` 会报 degraded。

## keybindings.json

一个数组，每条把某个上下文里的一个键（或空格隔开的和弦）绑到一个动作，`action` 为 `null` 是解绑：

```json
[
  {"context": "jobs", "keys": "x", "action": "delete"},
  {"context": "jobs", "keys": "d", "action": null},
  {"context": "global", "keys": "Ctrl+Q Ctrl+Q", "action": "quit"}
]
```

上下文：`global` `composer` `transcript` `sessions` `flows` `jobs` `dialog` `picker` `instances` `overlay`。
动作名是 `Ctrl+H` 帮助里列的那些（kebab-case）。逐条叠在内置键位上；跟别的绑定冲突的那条不生效，原因在
`/doctor` 里，别的照用。
