# Conix 使用手册

Conix 是跑在终端里的 agent 工作台。一个项目一个**实例**（常驻后台的 `conix host`），引擎 Agix、流程、
作业都跑在实例里；终端界面（TUI）接入实例，退出时可以让实例继续跑。远程使用就是 ssh 到那台机器再运行
`conix`，Conix 不监听任何网络端口。

| 章 | 内容 |
|---|---|
| [实例](instances.md) | 新建、接入、脱离、停止；`conix list`；崩溃后自动拉起；日志 |
| [工作台](workbench.md) | 界面的三块、键位、对话、会话标签（含已关闭的会话）、回滚、复制、`@` 提及、斜杠命令 |
| [流程](flows.md) | 流程定义与运行、运行操作、已结束的运行 |
| [作业与护栏](jobs.md) | 定时作业、在对话里建与改、护栏、停下与重试、审计摘要 |
| [插件](plugins.md) | `/plugin` 装、披露、启停；插件带来的流程、门禁与步骤；宿主库 `conix.*` |
| [通知](notify.md) | 有事等你时经命令送出去；没配时的「离开期间」 |
| [配置与文件](config.md) | `conix.yaml` 全部字段、`keybindings.json`、项目里的 `.conix/` 哪些进 git |

设计见 `docs/design/`，需求见 `docs/reqs/`。
