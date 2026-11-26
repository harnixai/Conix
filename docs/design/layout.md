# 项目里的目录：脚手架、运行、归档

一个项目里，Conix 的东西分三处：**`.conix/` 放脚手架**（流程本身：定义、件、命令、规矩），**`conix/runs/`
放运行中的东西**（不进 git），**`conix/<流程包>/` 放运行完归档下来的东西与流程包的长期数据**（进 git）。
全局目录与 Agix 的数据目录见 architecture.md §8.1；这里只讲项目里流程相关的部分。

## 1. 布局

```
<项目>/
├── .conix/                               脚手架：进 git
│   ├── flows/<包>/                        一个流程包：自洽，整个目录拷走就能用
│   │   ├── bundle.yaml                    包的说明（有它才是一个包）：流程、选路、步进、命令
│   │   ├── <流程>.yaml …                  一个或几个流程：同一件事的几条路径（比如 solid / quick）
│   │   ├── skills/<名>/SKILL.md           local.md：用户在后面补的一节（§4）
│   │   ├── gates/  scripts/  policies/  rules/  hooks/
│   │   ├── lib/<名>.js                    包内共用的 JS：脚本与命令 import，不出包（flows.md §7.6）
│   │   ├── templates/                     制品的骨架（站的 deliverables 引用）
│   │   └── commands/<名>.js               包自己的命令（QuickJS）：会话经 flow_command、人经 /… 命令、站经命令步骤调
│   ├── flows/*.yaml、flows/skills/ …      旧的平铺布局：照旧能读（§5）
│   ├── governance/                        项目的原则、治理文件：流程的门禁读它
│   ├── roles.yaml  jobs.json  settings.json …
│   └── state/                             本机数据（Agix 的），不进 git：会话、运行账本（flow-runs/）
└── conix/                                 制品；目录名可改（conix.yaml 的 flows.artifacts_dir）
    ├── .gitignore                         Conix 第一次起运行时写：runs/
    ├── runs/<运行>/                        一次运行的工作目录：不进 git
    │   ├── <制品>                          站写的交付物（包里流程的 ${output_dir} 就是这里）
    │   └── _run/                          流程库的投影：task.json、flow.yaml、stations/<站>/…、门禁日志
    └── <包>/                              这个包的长期数据：进 git
        ├── specs/ …                       跨运行的数据，比如规格基线：包的命令维护（${data_dir}）
        └── archive/<归档名>/               一次运行结束后归档：留下的制品 + record.json
```

## 2. 流程包

- **自洽**：一个包里的流程只用这个包的件。流程里写的名字（`skill: propose`、`gate: spec-delta`、`policies: [x]`）
  读成这个包的；包里没有就是缺件，校验时报出来，不往别的包或内置的件里找。几条路径要共用的件放在同一个包里。
- **名字**：包里的流程对外叫 `<包>/<文件名>`（`xforge/solid`）；文件叫 `flow.yaml` 时就叫包名（`brief`）。件也是
  `<包>/<名>`。流程文件里 `name:` 写文件名（`flow.yaml` 写包名）。
- **`bundle.yaml`**：`name`（与目录名相同）、`title`、`description`、`version`，以及 `flows`（选路的顺序）、`start`（`route`、`triage`）、
  `pace`、`commands`（`read_only` 声明）；字段见 flows.md §7.1。目录里有它才当一个包读；没有它的子目录
  （`skills/`、`gates/` …）是旧的平铺布局。
- **来源与覆盖**：包可以随 Conix 内置、随插件来、或在项目的 `.conix/flows/` 里。项目里有**同名、带 `bundle.yaml`** 的包，
  整包代替其他来源的同名包；项目里同名目录**不带 `bundle.yaml`**，只是给它加 `local.md`（§4）。
- **`lib/`**：包内共用的函数。脚本与命令用相对路径 `import`（`../lib/specs.js`），只认包内，不出包、没有 npm，随运行一起钉住。
  门禁与命令共用一份判法放在这里。
- **名字一个空间**：包里的流程名、命令名、站 id 不能重名，`help`、`status` 保留。地址里这三种名字共用一个位置，见下。
- **命令**：`commands/<名>.js` 是包自己的命令。站里的会话用 `flow_command {command, args}` 调；命令读得到项目，只写得了
  这次运行的工作目录（`_run/` 除外）和包的数据目录 `conix/<包>/`。结构化的数据（规格增量、台账）由命令写，模型不手写。

- **导出与导入**：包可以导出成 `<包>-<版本>.zip`（带 `MANIFEST.yaml`：导出它的 Conix 版本、要求、每个文件的 SHA-256）、
  装进别的项目；`export`、`import` 是包的保留名。见 flows.md §7.12。

### 2.1 地址

包里的东西用 `<包>:<名字>` 指：

| 写法 | 在哪能用 | 指什么 |
|---|---|---|
| `<包>/<流程>`（`xforge/solid`） | 流程定义、`flow_start` | 一条流程；`flow.yaml` 叫包名 |
| `<包>/<件>` | 流程定义里引用 | 包的件（skill、门禁、脚本、策略） |
| `/flow:<包>[:<名字>]` | 全局的斜杠命令 | 名字是流程、站或命令，按包里的类别定 |
| `/<包>:<名字>` | 流程上下文的斜杠命令 | 同上，运行就是当前对话所在的运行 |

名字是哪一类由包里的名字空间定，规则见 interaction §4.5。

## 3. 运行与归档

- 包里的流程不写 `output_dir`：运行的工作目录就是 `conix/runs/<运行>/`，制品写在这里，流程库的投影在 `_run/`。
  `runs/` 整个不进 git：进行中的东西、日志、上下文、临时文件都在这里。
- **归档**：运行正常结束时，流程库把留下的制品拷到 `conix/<包>/archive/<归档名>/`，写一份 `record.json`。
  - 留下哪些：流程的 `archive.keep`（相对工作目录的路径或 glob）；不写就是各站 `deliverables` 声明的全部；
    都没有就是工作目录里 `_run/` 以外的全部。
  - 归档名：`archive.name` 模板（`${date}`、`${seq}`、`${flow}`、`${title}`、`${inputs.<k>}`），缺省
    `${date}${seq}-${flow}`（`flow` 是文件名那段）。
  - `record.json`：主题、输入、各站结论与每道门禁的结论与证据（hash，路径换成归档里的）、确认（谁、何时、理由、经由）、
    交付出处、用量与耗时。租约、token、日志、上下文不进。
  - 取消的运行不归档，留在 `runs/` 里；`archive.when: always` 时也归档，`record.json` 的状态是 `cancelled`。
  - `workspace: worktree` 的运行在它的 worktree 里归档，跟代码一起合回去。
  - **事后核对**：`record.json` 带 `format` 和 `files: [{path, sha256, size}]`，列出归档目录里除它自己以外的每个文件
    （相对路径、`/` 分隔、排序，格式同导出包的 `MANIFEST.yaml`）；运行账本记下 `archive: {dir, record_sha256}`
    （`dir` 相对项目，`record_sha256` 是写完时 `record.json` 的摘要）。账本模型写不了，所以 `record.json` 连同 `files`
    被改写也看得出来。**不做哈希链**：归档提交进 git，历史加账本里这一个摘要，批过的制品之后被改就看得见，
    链只会多出一套要维护的东西。
  - `conix flow verify <归档目录> [--project 目录]`（不要实例，直接读项目的运行账本，调 `harnix_flow::archive::verify`）报告：`missing`（列了没有）、
    `extra`（有了没列）、`changed`（摘要或大小不同）；门禁证据与确认绑定的摘要对着归档里的文件重算，不符的列出；
    `record` 是 `matches` / `differs` / `no_ledger`（账本里没有这次归档，如旧的）。没有 `files` 的旧归档只报
    `unlisted`、仍核对证据摘要——这是核对不了，不算失败。
- 权威的运行账本仍是 `.conix/state/conix/flow-runs/`（带租约与 token，模型写不了）；`_run/` 与 `record.json` 都是它的投影。
- 模型能写 `conix/runs/<运行>/`，不能写 `_run/`；`conix/<包>/` 只经包的命令与归档写。

## 4. 制品声明与 skill

- 站声明它的制品：`deliverables: [{path, template?, description?}]`，路径相对工作目录。
  - 协议把它们列给会话：要交哪些文件、哪些从骨架开始。
  - 有 `template`（包的 `templates/` 下的文件）的，进站时放进工作目录（已有就不动）；骨架里的
    `<!-- conix:template -->` 是标记。
  - 每站隐含一道检查：声明的文件都在、不空、标记已去掉。不用再为"写没写"单写门禁。
- skill 只写这一站怎么干活（流程协议已经说了交什么、怎么交、怎么判）。用户要补充，写 `skills/<名>/local.md`：
  读 skill 时接在后面（"本项目的补充"），包升级时它留着。

## 5. 旧布局

- `.conix/flows/*.yaml` 与 `.conix/flows/{skills,gates,…}/` 的平铺布局照旧能读：流程名、件名不带包名，件按原来的
  层次找，`output_dir` 照旧相对项目根。它们的投影也挪进 `conix/runs/<运行>/_run/`；不归档。
- 插件的流程包目录（`[x-conix] flows`）里也可以放包（带 `bundle.yaml` 的子目录），和平铺的并存。
