# Conix

Conix（Console + *nix）是 Agent native 的 console shell：跑在终端里的 agent 工作台，引擎是 [Agix](https://github.com/harnixai/Agix)。左边是对话区，右边是三个标签：会话（主 Agent / 子 Agent / 任务）、流程、作业。可以本地用，也可以远程用，不限定于编程场景。

使用手册见 [docs/manual/](docs/manual/README.md)。开发约定见 [CLAUDE.md](CLAUDE.md)，需求与设计见 `docs/`。

```sh
git clone --recurse-submodules <repo>
git -C agix checkout main
```

在项目目录里运行 `conix`，就接入这个项目的实例；实例没在跑就先拉起它（`conix host`，常驻后台，
引擎 Agix 嵌在里面）。退出时选"后台运行"，实例继续跑，下次运行 `conix` 接回。模型与凭据沿用
Agix 的配置：`~/.conix/settings.json`，或环境变量 `ANTHROPIC_API_KEY` / `ANTHROPIC_AUTH_TOKEN` /
`ANTHROPIC_BASE_URL`。

构建：

```sh
git submodule update --init agix
cargo build -p conix
cd ~/my-project && /path/to/conix   # 接入（或新建）这个项目的实例
conix list --all                    # 本机的实例
conix attach <名字> --force         # 接入别处正接着的实例，踢掉对方
conix stop <名字>
conix --mock                        # mock 演示 demo：不起实例、不调模型
```

想直接看到各个部件有数据的样子：`scripts/demo.sh` 生成 `demo/`（不入库）——独立的
`CONIX_HOME`、一个示例项目，凭据取 `../Agix/.env`，再用真实模型跑几段对话、设一个 goal、
建三个作业（其中一个是在会话里让模型用工具建的），再起一次内置流程的运行（停在待确认）。之后
`demo/live.sh` 打开真实实例，`demo/mock.sh` 打开 mock 演示，`demo/stop.sh` 停掉，
`scripts/demo.sh reset` 从头再来。
