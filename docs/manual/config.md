# 配置与文件

## 配置在哪：总表

| 配置面 | 在哪 | 管什么 | 谁读、什么时候读 |
|---|---|---|---|
| Agix 设置（全局） | `~/.conix/settings.json`，个人的 `settings.local.json` | 模型与 provider、权限、沙箱、web、回合与会话预算…… | 引擎；随写随读 |
| Agix 设置（项目） | `<项目>/.conix/settings.json`，个人的 `settings.local.json`（不进 git） | 同上，只管这个项目 | 引擎；随写随读 |
| cli 层 | 实例启动时 Conix 写给引擎，没有文件 | 只读工具与流程工具不问、`client_may_loosen`、`app.history` | 实例启动；用户不改 |
| `~/.conix/conix.yaml` | 全局 | `ui`、`instance`、`jobs`、`notify`、`engine` | `ui` 由 TUI 启动时读；其余由实例启动时读 |
| `~/.conix/keybindings.json` | 全局 | 键位覆盖 | TUI 启动时 |
| 环境变量 | 进程环境 | `CONIX_HOME`、`CONIX_LANG`、`CONIX_MODEL`、`CONIX_PLUGIN_DIRS`、`RUST_LOG` | 进程启动时 |
| 插件 | `~/.conix/plugins/`（装好的）、`~/.conix/plugin-store/`（自己放的包） | 见 [插件](plugins.md) | `/plugin` |
| 项目 `.conix/` 里 Conix 的东西 | `flows/`、`jobs.json`、`state/conix/` | 流程与作业的定义、运行与状态 | 见「文件在哪」 |

改了文件要等"下次启动"才生效，除非走会话里的配置工具（architecture §8.11）：它改 `conix.yaml` 时，`jobs` 与
`notify` 实例当场重读，`ui`、`instance`、`engine` 下次启动生效。

## 文件在哪

```
~/.conix/                       全局（CONIX_HOME 可改）；也是 Agix 的数据根
  settings.json                 Agix 的设置：模型、provider、权限……（见「Agix 的设置」）
  settings.local.json           同上，这台机器的个人部分
  conix.yaml                    Conix 自己的配置（下面）
  keybindings.json              键位覆盖（下面）
  plugins/  plugin-store/       装好的插件（Agix 管）；自己放的插件包
  config-backups/               会话里改配置前的备份
  instances/<名字>/             实例的运行时文件：socket、锁、发现文件、日志
<项目>/
  .wkts/<运行>/                 流程运行的 worktree（不进 git）
  .conix/
    settings.json  agents/ …    项目的引擎配置（进 git）
    flows/                      项目自己的流程（进 git）
    jobs.json                   作业定义（进 git）
    settings.local.json …       个人配置（不进 git）
    state/                      本机数据与过程数据（不进 git）
      history/                  受信项目的会话
      conix/                    Conix 的命名空间，只认这台机器自己写的：
        sessions.json           会话归属
        jobs-state.json         作业在这台机器上的启用状态与调度基线
        jobs/<作业>.jsonl       触发记录
        usage/<YYYYMMDD>.json   每日用量
        flow-runs/              流程运行的记录
        runs/<运行>/            运行的过程目录（交付、门禁日志）
        run-policies.json       流程作业起的运行用的网络策略
        quarantine/<时刻>/      认不出来的状态
```

`.conix/.gitignore` 由 Agix 维护（`state/`、`settings.local.json`、`mcp.local.json`）。流程运行的过程目录在
`state/conix/runs/` 下，跟着 `state/` 不进 git；以前版本留在 `.conix/runs/` 的不再读，可以删掉。
`state/conix/` 只认这台机器自己写的：从仓库里带来的会被挪进 `state/conix/quarantine/`，不读。

## conix.yaml

不存在时会写一份带注释的缺省配置（按 `CONIX_LANG` 选中英文模板）。读不懂的项用缺省，原因在 `/doctor` 与日志里。
`ui` 由 TUI 启动时读，`instance`、`jobs`、`notify`、`engine` 由实例启动时读；改了下次启动生效（会话里的配置工具另说，见总表下的说明）。

```yaml
ui:
  language: zh              # zh | en | auto（照 LC_ALL / LC_MESSAGES / LANG）；CONIX_LANG 优先
  initial_size: 160x45      # 启动时请求的终端尺寸（列x行）；off 不请求
  ratio: 60                 # 宽屏时左栏占的百分比，40–75
  compact_tool_lines: 3     # 精简模式下工具结果显示几行
  syntax_theme: base16-ocean.dark
  theme:
    preset: claude          # claude | aqua
    colors: {}              # 逐项覆盖：accent border ok warn error …

instance:
  idle_timeout: 30m         # 空闲多久自动脱离；0 不超时
  personal_project: ~/Documents/Conix   # 个人项目；不写就是文档目录下的 Conix

jobs:
  defaults:                 # 作业没写时用
    timeout: 30m
    max_tokens: 2M          # 一次触发的 token：未命中缓存的输入 + 缓存写入 + 输出（缓存命中不计）
    overlap: skip           # skip | queue | replace
    disable_after: 3        # 0 不自动停用
    network: deny_all       # 作业里命令的出网：deny_all 不出网 | allowlist 只到 web.allowed_domains | inherit 照设置
  limits:                   # 天花板：作业定义写得再大也不超过它（网络没有天花板）
    timeout: 4h
    max_tokens: 20M
  daily_tokens: 50M         # 所有无人值守的工作一天合计；0 不限
  max_concurrent: 2         # 同时在跑的触发

# notify: …                 # 缺省不发，见 [通知](notify.md)

# engine:
#   model: claude-sonnet-4-6   # 只在 settings.json 没定义 providers 时用；CONIX_MODEL 优先
```

| 段 | 字段 | 缺省 | 说明 |
|---|---|---|---|
| `ui` | `language` / `initial_size` / `ratio` / `compact_tool_lines` / `syntax_theme` / `theme.preset` / `theme.colors` | 如上例 | 界面；时长、颜色、主题读不懂就退回缺省并报告 |
| `instance` | `idle_timeout` | `30m` | 写法 `45s` `30m` `2h` `1d`，`0` 不超时 |
| `instance` | `personal_project` | `<文档目录>/Conix` | 在家目录、它的上级或 `~/.conix` 里运行 `conix` 时进的项目（[实例](instances.md#个人项目)），第一次用就信任；`~` 开头按家目录展开。TUI 与实例启动时读 |
| `jobs` | `defaults.*`、`limits.*`、`daily_tokens`、`max_concurrent` | 如上例 | 作业护栏；数字与 `2M`、`500k`、`30m` 都收 |
| `notify` | `when`（`detached` 缺省 / `always`）、`min_level`（`info` / `warn` 缺省 / `error`）、`sinks[]`（`name`、`command`、`stdin`、`min_level`） | 不配就不发 | 只认这个全局文件 |
| `engine` | `model` | 不写，用 Agix 的缺省 | 引擎自带 `anthropic` 的模型（相当于 `agixd --model`）；是进程参数而不是 Agix 的设置键，所以放这里 |

## Agix 的设置

模型、provider、权限、沙箱、web、回合与会话预算这些是 Agix 的 `settings.json`：全局 `~/.conix/settings.json`（Conix 的家目录就是
Agix 的数据根），项目 `<项目>/.conix/settings.json`（进 git），两处都可以有同目录的 `settings.local.json`（这台机器的个人部分，不进 git）。
引擎随写随读，不用重启实例。键与含义见 [Agix 设置](../../agix/docs/settings.md)，这里不重复；会话里问引擎某个键现在是什么、
从哪来用 `config.explain`。

- **0.4 不再读旧键**（`model.model_name`、`execution.*`、`permission_rules` 之类）：读出来是空，不报错。升级后跑一遍 `/doctor`，
  Agix 的 `settings.tiers` 一项会列出 `旧 → 新`，被丢掉的值也在里面。迁移说明见 [升级到 0.4](../../agix/docs/upgrading-0.4.md)。
- **模型**：`providers` 是列表，缺省模型取 `model.order` 第一个 provider 的 balanced 档（没写取 strong 档）。没定义 `providers` 时用
  Agix 自带的 `anthropic`，它的模型在 `conix.yaml` 的 `engine.model` 里指定，环境变量 `CONIX_MODEL` 优先（都不写就是 Agix 的缺省）。
  要按项目或档位选模型，就定义 `providers`。
- **Conix 在 cli 层写的**：实例启动时作为 cli 层交给引擎（`host::hub::cli_settings`，`app.history` 在 `engine` 里补上），用户不用也不该去改。
  内容：`permissions.allow` 放行只读工具与流程自己的工具，免得每次都问；
  `permissions.client_may_loosen: true`，让流程的站能用 `acceptEdits`（0.4 的会话选项只能收紧，不开这个就被夹回，无人看着的运行会停在权限提问上）；
  `app.history: "project"`，受信项目的会话放进项目里。这个开关是 Agix 说的"安全缺省"而不是安全边界，部署方的 managed 设置仍然压过它，
  能把它关掉；关了 `/doctor` 的 `conix.session_loosen` 会报 degraded。

## 在会话里改配置

可以直接让会话里的模型改 Conix 的配置，比如"加一个 DeepSeek 的 provider，测通了再切过去"。它用的是六个工具，
只有你自己开的会话有，作业与流程里的会话没有；会改东西的几个照常先问你。

| 工具 | 做什么 |
|---|---|
| `config_show` | 现在的缺省模型、路由（`model.order`）、每个 provider（端点、key 放在哪个环境变量、在不在）、`conix.yaml` |
| `config_provider_add` | 加一个 provider，**不切换**：路由不变，会话照常用现在的模型 |
| `config_provider_test` | 测通：先问端点，再开一个临时会话用它答一句（不进会话列表，测完删掉） |
| `config_switch` | 切过去：先测，不通不切；备份现在的路由；切完立刻再答一句，不通自动退回 |
| `config_rollback` | 退回上一次切换前 |
| `conix_config_set` | 改 `conix.yaml` 的一项：先校验（读不懂的值直接拒），备份后写；`jobs`、`notify` 当场生效，其余下次启动生效 |

为什么要分"加、测、切"：改了立刻生效的话，新的 provider 不通，会话当场就没有模型可用，也就没法再改回来。

- **key 不写进设置**：provider 只写 `api_key_env`（放 key 的环境变量名）。实例进程要看得到这个变量。
- **只有自带 provider 时**：设置里一个 provider 都没定义时，引擎用自带的 `anthropic`；一旦定义了任何一个，自带的
  就没了。所以加第一个 provider 之前，Conix 先把自带的照原样写成显式的 `anthropic` 并钉住路由，测一句通了才继续，
  不通就撤回、什么都不加。自带的本来就不通（还没给 key）时没什么要保住的，加了就走新的。
- **备份**：切换前的路由在 `~/.conix/config-backups/<时刻>.json`，`conix.yaml` 的旧文件在
  `config-backups/conix-<时刻>.yaml`。`conix_config_set` 重写文件时不保留注释，注释在备份里。

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

## 环境变量

| 变量 | 作用 |
|---|---|
| `CONIX_HOME` | 全局目录，缺省 `~/.conix`；也是 Agix 的 `data_root` |
| `CONIX_LANG` | 界面语言（`zh` / `en`），压过 `ui.language`；也决定首次写出哪种语言的 `conix.yaml` 模板 |
| `CONIX_MODEL` | 引擎自带 `anthropic` 的模型，压过 `engine.model`；设置里定义了 `providers` 时无效 |
| `CONIX_PLUGIN_DIRS` | 自带插件目录，`:` 分隔，替换 `<程序目录>/../share/conix/plugins/`（开发与测试用） |
| `RUST_LOG` | 日志过滤，写进 `instances/<名字>/host.log` |

`CONIX_HOST_CHILD` 是看守进程给实例子进程设的内部标记，不用设。

**开发与测试**：`CONIX_TEST_*`（如 `CONIX_TEST_NOTIFY_WINDOW_MS`、`CONIX_TEST_HEARTBEAT_TIMEOUT_MS`，缩短时间窗）、
`CONIX_UPDATE_SNAPSHOTS`（重写快照）、`CONIX_LIVE_ENV`（LIVE 测试的环境文件）、`CONIX_CONFORMANCE`（流程一致性用例）只在测试里读。

