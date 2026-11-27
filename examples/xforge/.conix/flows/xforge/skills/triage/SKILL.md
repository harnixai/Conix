---
name: triage
description: 判一个变更的风险与影响面，选 xforge 的路径并启动。
---

用户要用 xforge 做一个变更，但没说风险。先读代码再判，不凭主题的字面：要改哪些文件和模块、谁在调它们、测试盖到没有、
`xspec list`、`xarch modules` 里牵到哪些需求与接口。

风险（`risk`）：
- `low`：改动小而局部（一两个文件），不碰已有的对外行为，出错容易发现、容易退回。走 quick：只有开发线。
- `medium`：常规的功能或修改，跨几个文件，改已有行为但范围清楚。走 solid：开发线与测试线，规格定了分两路，
  一路实现、一路只照规格写黑盒测试，合起来用测试跑实现。
- `high`：跨模块或跨系统，动了对外接口、数据格式、鉴权，或者退回很难；需求本身还有没定的地方。走 major：先澄清，
  再加一条与开发线不同模型的检视线，各设计阶段检视。

影响面（`impact`，可以多选，没有就是 `[]`）：动了对外接口（签名、行为、协议）`interface`，动了鉴权、权限、敏感数据 `security`，
要改已有数据的格式或搬数据 `data-migration`，动了部署、配置、CI `infra`。
要改 `xarch list` 里 stable 的元素（签名或行为），impact 一定带 interface：破坏兼容的改动没有它过不了设计。
影响面不改变路径，但每一项都要在提案里交代；动了接口而接口基线关着，告诉用户。

拿不准就选长的那条：选短了中途要换路或重来，选长了只是多几站。

solid、major 要项目的 `.conix/governance/xforge.yaml` 的 verify 菜单有 `blackbox` 一项（黑盒测试放哪、怎么跑），没有就告诉用户先加，
或者先用 quick。major 的检视线要治理文件的 `lines.review.model` 写一个与开发线不同的模型，没写第一站就停下：告诉用户先写。

判完启动：`flow_start {flow: "xforge", subject: "<主题>", inputs: {title: "<短标题>", risk, impact}, reason: "<凭什么这样判：改哪些、影响谁>"}`。
标题短，也是归档的名字。
