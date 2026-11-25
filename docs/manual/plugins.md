# 插件

插件是一个带 `plugin.toml` 的包（zip）。包里的命令、函数、脚本、skill、agent 由引擎 Agix 兑现；包里写给
Conix 的 `[x-conix]` 一段由 Conix 读：它带来的流程包、host 门禁与程序步骤各由哪个插件函数做。设计见
`docs/design/plugins.md`，包的格式见 Agix 的 `docs/plugin_packages.md` 与 `docs/runext.md`。

## 从哪装

`/plugin` 列出两处的包，只按文件名 `<名>-<版本>.zip` 认：

- **自带目录**：`<conix 所在目录>/../share/conix/plugins/`，随程序装上；开发与测试时用 `CONIX_PLUGIN_DIRS`
  （`:` 分隔）换掉它。`scripts/build-plugins.sh` 把仓库里的 `plugins/*` 打到 `target/plugins/`。
- **你自己放的**：`~/.conix/plugin-store/`。

装好的插件放在 `~/.conix/plugins/cache/`，启停状态在 `~/.conix/plugins/enabled.json`，都归 Agix 管，对所有项目生效。

## `/plugin`

| 写法 | 做什么 |
|---|---|
| `/plugin` | 浮层：装好的（启用与否、带来的流程、冲突）在前，可装的在后 |
| `/plugin install <名或 zip 路径>` | 装，然后看披露、决定启不启用 |
| `/plugin enable <名>` / `disable <名>` | 启停 |
| `/plugin remove <名>` | 卸载 |
| `/plugin info <名>` | 再看一次披露 |

浮层里：Enter 装（可装的）或看披露（装好的），空格启停，`d` 连按两次卸载，Esc 关。

**装完是停用的。** 浮层摆出两份披露，确认（`y` / Enter）才启用，不确认（`n` / Esc）就卸掉：

- **Agix 的**：命令能写什么、给宿主调的函数、绑在哪个扩展点上的脚本、用了宿主库 `conix`、要起的 MCP 服务器，
  以及每一段会一字不差到达模型的文字（插件描述、agent 提示词、skill、命令描述）。
- **Conix 的**：它带来的流程、host 门禁与程序步骤各对应哪个插件函数、流程包里到达模型的 skill 与规则。

文字到达模型是隔离挡不住的，唯一的防线是有人去读。

**启停之后**：插件的命令只进此后新建的会话；它带来的流程立刻出现在流程标签里（来源写「全局 · 插件 <名>」），
停用就撤下。已经在跑的运行不受影响：定义、策略、门禁在启动时就钉在运行记录里。

## 插件带来的流程

- 叠加次序：项目自己的 > 插件（按插件名）> 自带的示例。项目里存同名的就是覆盖，删掉
  回到插件那份。两个插件有同名的流程、门禁或步骤，插件名排在前面的胜出，另一个的在 `/plugin` 里报冲突。
- 插件的 host 门禁与程序步骤经引擎的 `plugin.call` 调插件函数，不带会话，在运行干活的目录里跑：门禁只读，
  步骤只写得了函数声明的范围，时限缺省 120 秒；插件可以给某道门禁或某个步骤写更长的，至多 10 分钟，安装时的披露里会写明（"up to 8 min"）。**不带会话的调用要问人的一律失败**：门禁里要跑命令的（比如
  `git status`），在项目的 `.conix/settings.json` 里给它写 `permissions.allow` 规则，门禁没过时 `detail` 会写明要哪条。

## 插件里能用的宿主库 `conix.*`

包顶层写 `uses = ["conix"]`，它的命令与函数里就有全局 `conix`：

| 函数 | 做什么 |
|---|---|
| `conix.flow.defs()` | 这个项目能用的流程（名字、标题、来源、插件名） |
| `conix.flow.runs({flow?, active?})` | 运行列表 |
| `conix.flow.run({run})` | 一次运行：状态、各站与输出 |
| `conix.flow.current()` | 调用所在的运行与站；不在运行里为 `null` |
| `conix.flow.start({flow, subject, inputs})` | 开一次运行，回 `{run}` |
| `conix.plugin.call({function, input})` | 调本包自己的插件函数（不带会话）：流程站的会话里命令写不了文件（写要逐次问派工器，命令问不了人），写数据交给函数 |

能做什么看是谁在调，不看包名（包名是包自报的）：门禁与步骤里、没有会话时、作业的会话里只能读；流程站的会话
只看得到自己那次运行；只有人开的会话能开运行。

## 随 Conix 的插件：xforge

规格驱动的变更：提案与规格增量、台账、工作包、门禁，按流程推进。数据在项目的 `xforge/` 下，随项目提交。

1. `/plugin install xforge`，看过披露后启用。
2. 在对话里让模型跑 `xforge init`（建 `xforge/manifest.yaml` 与 `constitution.md`），把验证命令写进 manifest 的
   `verification.commands`。
3. 开一次变更：`xforge start quick "<标题>"`（低风险：提案 → 实施 → 验证 → 归档）或
   `xforge start solid "<标题>" --risk medium`（多设计、检查、按工作包实施）。也可以在流程标签里直接开
   `xforge-quick` / `xforge-solid`，输入写 `title` 与 `risk`。
4. 运行走到验证站时停下等你确认；确认后归档：规格增量并进 `xforge/specs/`，变更拷进 `xforge/archive/`。

模型直接写的只有 `xforge/changes/<id>/` 下的 `proposal.md`、`design.md`、`verification.md`；规格增量、台账、工作包
只经 `xspec` / `xflow` 命令写（守卫挡住直接改）。门禁 `xforge-unclaimed` 跑 `git status`，`xforge-unit-tests` 跑
manifest 里的验证命令：要在项目设置里给它们 `permissions.allow` 规则，没给时门禁不过并写明要哪条。
数据格式与七道门禁的判定见 `docs/design/plugins.md` §6.6。
