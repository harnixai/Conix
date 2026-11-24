# 通知

实例里有事等你处理时——会话在等你回答、运行的站等确认或搁置、作业失败或被护栏停下、作业被自动停用、
每日额度用完——Conix 可以经你配置的**命令**把它送出去。

## 配置

只认全局的 `~/.conix/conix.yaml`（项目里的文件改不了它：不能让一个仓库在你的机器上执行命令、把数据往外发）。

```yaml
notify:
  when: detached        # detached：没有 TUI 接着时才发（缺省）；always：一直发
  min_level: warn       # info | warn | error
  sinks:
    - name: desktop     # macOS 桌面通知
      command: ["terminal-notifier", "-title", "Conix {instance}", "-message", "{title}"]
    - name: phone       # ntfy 推到手机
      command: ["sh", "-c", "curl -s -d @- https://ntfy.sh/$CONIX_NTFY_TOPIC"]
      stdin: json       # 整条通知以 JSON 写进 stdin
      min_level: error
```

- 占位符：`{instance}` `{project}` `{kind}` `{level}` `{title}` `{text}` `{attach}`（`attach` 是回来的那句
  `ssh <主机> -t conix attach <名字>`）。只做参数替换，不经 shell；要 shell 就自己写 `["sh", "-c", "…"]`。
- 一分钟里来的几件事合成一条送；同一件事只送一次。
- 每条命令至多 10 秒，失败重试一次；送过的记在 `<项目>/.conix/state/conix/notify.jsonl`。
- `/notify` 发一条测试通知，底栏写出每个出口成没成。

## 离开期间

没配通知时什么都不发。下次接入时弹出「离开期间」，列出这段时间攒下的事，`Enter` 直接跳过去处理（与
`Ctrl+R` 同一个落点），`Esc` 关掉。配了通知的照样有它。
