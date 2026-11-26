---
name: clarify
description: 把会改变范围或设计的问题写下来，交给人回答。
---

读这次运行目录里的 `proposal.md`、`xspec delta`、`xarch delta`，以及要动的代码。

只有答案会改变范围、需求或设计的才是要问的问题；好奇、你自己读代码就能查到的不是。每个问题一条：
`xledger add questions "<问题>。默认：<你建议的答案>" --ref <它影响的提案小节或需求编号>`。
给出默认答案，人只需要同意或改；一条只问一件事。没有要问的就不记，summary 里写明为什么没有。

问题只有人能回答，你不给结论，也不改提案和增量。记完交出（`questions` 是问了几条），并告诉用户：
每条用 `/xforge:xledger set questions <编号> answered "<回答>"`（或 `deferred "<为什么以后再说>"`）答，
都答完打 `/xforge:clarify` 重判这一站，再 `/confirm`。答案改了范围的，人会把运行打回提案。
