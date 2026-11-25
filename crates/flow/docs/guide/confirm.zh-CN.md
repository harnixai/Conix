# 确认与授权

门禁判"交付合不合格"，确认判"人同不同意"。需要人拍板的地方写一个 `confirm` 出口：任务里的会话没有提问工具，
模型不会停下来问人，也**不能替人确认**——确认、放行搁置的站、强制放行都只有人能做，任何工具调用都做不了。

## 人在应用里确认（`via: person`，缺省）

```yaml
  - id: sign
    title: 签字
    exit:
      - kind: confirm
        who: reviewer        # 可选：角色，交给应用解释（谁算 reviewer 由应用定）
        label: 负责人签字     # 可选：界面上显示
        timeout: 3d          # 可选：到时没有决定算不过（90s、30m、8h、3d、1h30m）
    rework_to: [draft]       # 驳回后人可以把运行退回到哪
```

- 站进来后等确认，运行显示"待确认"；人在应用里通过或驳回，可以写理由。
- 驳回就是这条出口不过：站未达标，写了 `on_unmet` 就自动打回，否则等人决定退回哪一站。
- `timeout` 到了还没有决定，同样不过，说明写"3d 内没有决定"。
- 每个决定都记下谁、什么时候、第几轮、通过与否、理由。

## 外部审批系统（`via: mcp`）

要"别人"批（主管、法务、另一个团队），而他们不在这个应用里时，经 MCP 工具在外部审批系统（飞书、钉钉、Jira……）建单，
之后轮询它的状态：

```yaml
      - kind: confirm
        who: reviewer
        via: mcp
        timeout: 3d
        mcp:
          server: feishu                          # 应用里配置好的 MCP server
          request:                                # 进站时调一次：建单
            tool: approval_create
            arguments: {title: "${subject}：${station}", approver: "${inputs.reviewer}", run: "${run}"}
          status:                                 # 之后每隔 poll 调一次：查单
            tool: approval_get
            arguments: {id: "${request.id}"}     # ${request.*} 是建单返回的结果
          decision:                               # 从查单结果里判
            path: status                          # 结果里的字段路径
            approved: [APPROVED]
            rejected: [REJECTED, CANCELED]        # 其他取值：还在等
          by: approver.name                       # 可选：结果里谁批的
          link: url                               # 可选：单的链接，记下来给人看
          poll: 5m                                # 缺省 5m，至少 30s
```

- 参数里能引用 `${run}`、`${subject}`、`${station}`、`${inputs.*}`、`${stations.*.output.*}`；`status` 的参数还能引用
  `${request.*}`。
- 建单每一轮只建一次，单号记在运行里：应用重启后接着查同一张单，不另建。
- 建单失败，或查单连续失败 5 次，这条出口是 `error`：不自动打回，人看过后重跑这一站的检查。
- 判定之后记下审批人（`by`）和单号或链接。
- 先确认 server 和工具名：应用里有哪些 MCP server、它们的工具叫什么、建单要哪些参数、查单返回什么字段，问用户或看
  server 的说明，不要猜。

## 强制放行与受管流程

- 人可以强制放行一站（写理由，记下当时哪些条件没满足），或把运行退回到某一站返工。
- 插件可以把它的流程声明为**受管**：项目里不能存同名的去顶替它，它用到的件只取插件自己的，它的运行**不能强制放行**。
  要改受管流程，改插件。

## 什么时候用哪个

| 场景 | 写法 |
|---|---|
| 交付能机械判 | 门禁（见 `flow_guide {topic: "gates"}`） |
| 要一个人看一眼、拍板 | `confirm`（`via: person`） |
| 要指定的人批，可能不在应用里 | `confirm` + `who` + `via: mcp` |
| 某次工具调用要人点头（写发布目录、删东西） | 权限 `effect: ask` |
| 进站前先等人放行 | 站上 `hold: true` |
