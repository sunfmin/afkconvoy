# afkconvoy 用户手册

[English](manual.md)

这份手册写给搭建并长期使用 afkconvoy 的人：fleet 的操作者，以及设计团队（角色和工作流）的人。它用一个贯穿始终的真实例子，带你从零做到「一个仓库的 issue 由一组角色协作完成，全程没有人在回路里」。

精确的参考资料是工具自己：`afkconvoy --help` 和 `afkconvoy <命令> --help` 写明了每条命令读什么、写什么、输出什么。术语定义在 [`GLOSSARY.md`](../GLOSSARY.md)，设计背后的理由在 [`docs/adr/`](adr/)。这份手册只讲该做什么、按什么顺序做。

> 术语保留英文原词，首次出现时给出中文解释：fleet（机群）、worker（工作者，即一个编码 agent）、tick（一轮调度）、gate（门禁）、landing train（落地列车）、role（角色）、team（团队）、workflow（工作流）、stage（阶段）、verdict（裁定）、flow label（流程标签）、patrol（巡查）、precedent（先例）、guard（守卫）。

## 目录

1. [afkconvoy 做什么](#1-afkconvoy-做什么)
2. [安装](#2-安装)
3. [简单用法：未绑定团队的仓库](#3-简单用法未绑定团队的仓库)
4. [团队：核心概念](#4-团队核心概念)
5. [完整示例：一家给代码看病的医院](#5-完整示例一家给代码看病的医院)
6. [运行 fleet](#6-运行-fleet)
7. [fleet 需要你的时候](#7-fleet-需要你的时候)
8. [怎样写好角色和工作流](#8-怎样写好角色和工作流)
9. [参考](#9-参考)
10. [出问题时](#10-出问题时)

## 1. afkconvoy 做什么

你有一个 GitHub 仓库，里面积压着一堆 issue。在它的 checkout 里启动 `afkconvoy run`，它就在你不在的时候把这些 issue 做掉：

- 对每一张就绪的 issue，启动一个编码 agent——称为 **worker**——在它独占的 git worktree 里工作；
- 盯着这些 worker：停下来的催一下，死掉的换一个；
- 每个完成的 PR 都通过**落地列车（landing train）**合入主干，合入前必须通过仓库的**门禁（gate）**——也就是那条说明「这棵代码树是好的」的命令；
- 某张 issue 实在走不下去时，在 issue 上评论说明原因，然后交还给你。

fleet 知道的一切都不存在你的机器上。谁在做哪张 issue、什么正在落地、一张 issue 走到了工作流的哪一步——全部是每个 **tick** 从 GitHub 和仓库远端现读出来的。所以你可以随时杀掉 `run` 再重启，也可以让多台机器同时处理同一个仓库。

它有两种用法：

| | 未绑定的仓库 | 绑定了团队的仓库 |
|---|---|---|
| 配置里写了 | 没有 team | 一个 team |
| issue 带上什么标签才会被处理 | ready 标签 | 流程标签 `flow:<工作流名>` |
| 一张 issue 由谁做 | 一个 worker，从头做到 PR | 每个**阶段**一个新 worker，各自在自己的**角色**里 |
| worker 是什么 | 你的启动命令拉起的任何 agent | Claude Code |
| 看哪里 | [第 3 节](#3-简单用法未绑定团队的仓库) | 第 [4](#4-团队核心概念)、[5](#5-完整示例一家给代码看病的医院) 节 |

只想把 issue 变成 PR，就从未绑定开始。想让代码在落地前被作者以外的角色评审，再把仓库绑定到一个团队。

## 2. 安装

macOS，Apple Silicon 或 Intel：

```sh
brew install sunfmin/tap/afkconvoy
afkconvoy --version
```

Homebrew 会顺带装好 `git` 和 `gh`。还有三样东西需要你自己准备：

| 什么 | 安装 | 登录 |
|---|---|---|
| [orca](https://github.com/stablyai/orca)，所有 worker 都通过它启动 | `brew install --cask stablyai/orca/orca` | — |
| GitHub | （已装） | `gh auth login`，用一个对仓库有 push 权限的账号 |
| Claude Code（用它启动 worker 时） | `curl -fsSL https://claude.ai/install.sh \| bash` | `claude auth login` |

orca 一定要写全 tap 名：Homebrew 自己那个叫 `orca` 的 cask 是另一个软件。orca 需要处于运行状态——打开应用，或者 `orca open`。

这些你不用自己逐项检查。`plan`、`tick`、`run` 会先检查，缺了哪个命令或哪个没登录，就什么都不做，并逐项打印该运行的那一行。afkconvoy 不替你安装任何东西，也不替任何人登录。

## 3. 简单用法：未绑定团队的仓库

**第 1 步：写配置。** 在仓库主干上添加 `docs/agents/afkconvoy.md`。只有文件里第一个 `yaml` 代码块会被读取：

````markdown
```yaml
base_branch: main          # 必填：worker 从它切分支，PR 落到它上面
gate_command: ./gate.sh    # 必填：只有代码树是好的才退出 0
concurrency: 3             # 同时有几个 worker（默认 3）
```
````

**第 2 步：把门禁做成一条命令。** 格式化、lint、测试——变更落地前必须成立的一切，放进一个脚本，全部通过才退出 0。它是 worker 和你的主干之间唯一的机器检查，所以必须值得信任。

**第 3 步：保持主干可以直接 push。** 落地列车的落地方式是把一个通过了门禁的提交 push 上去。要求评审或状态检查的分支保护会拒绝这种 push，fleet 在这样的仓库上会拒绝启动。

**第 4 步：标记 issue。** 给 fleet 可以拿走的每张 issue 打上 `ready-for-agent` 标签。GitHub 上记录为「被某张未关闭的 issue 阻塞」的 issue 会等它；带 `epic` 或 `prd` 标签的 issue 永远不会被拿走。

**第 5 步：先看，再跑。** 在仓库的 checkout 里：

```sh
afkconvoy config    # 读到了什么配置
afkconvoy plan      # 一个 tick 会从哪些 issue 开始；什么都不改
afkconvoy run       # 启动 fleet，直到你停掉它
```

每张 issue 只有三种结局：PR 落地、issue 关闭；worker 发现无事可做、issue 关闭；或者 fleet 把 `ready-for-agent` 换成 `ready-for-human`，并在评论里说明原因。照评论修好，再把 ready 标签打回去即可。

## 4. 团队：核心概念

一个 worker 自己计划、自己写、自己评，它一定会放自己过关。团队把这些工作拆给不同的**角色（role）**，再由**工作流（workflow）**规定一张 issue 按什么顺序经过它们。

```
        项目仓库                              团队仓库                        角色仓库
  docs/agents/afkconvoy.md            team.md        ── 指向 ──▶       CLAUDE.md          规则
     team: owner/team      ── 指向 ▶  workflows/fix.md                  .claude/skills/*   它能被要求做的事
                                      patrols/triage.md                 .claude/settings.json   hook：它永远不能做的事
```

**一个角色就是一个仓库。** 它就是一个普通的 Claude Code 项目目录：`CLAUDE.md` 写这类 worker 始终遵守的规则，`.claude/skills/` 下是技能，`.claude/settings.json` 里是 hook。处于某个角色的 worker，就是**在那个目录里启动**的 Claude Code，再把 issue 的 worktree 作为附加目录加进去。你手动打开角色仓库，进入的就是 worker 所在的同一个角色。worker 不加载你个人的任何 Claude Code 配置——角色就是它的仓库所写的样子，在每台机器上都一样。

**团队也是一个仓库。** 里面有 `team.md`，给每个角色起一个短名；`workflows/` 下每个工作流一个文件；`patrols/` 下每个巡查一个文件。每个文件只读第一个 `yaml` 代码块，其余部分留给你写说明。

**工作流是一串阶段（stage）。** 每个阶段指定一个角色，以及让该角色执行的那个技能：

```yaml
stages:
  - name: build
    by: dev              # 团队里的一个角色，或者 `human`
    do: implement        # 该角色仓库里的一个 skill
    on: { pass: review }
  - name: review
    by: reviewer
    do: review
    writes: false        # 这个阶段不允许提交代码
    on: { pass: land, reject: build }
land: { by: dev, do: land }
```

**issue 靠流程标签（flow label）进入工作流。** 文件 `workflows/fix.md` 就是工作流 `fix`，它的流程标签是 `flow:fix`。给 issue 打上这个标签，它就位于该工作流的第一个阶段。没有流程标签的 issue 不会被碰。

**每个阶段都是一个全新的 worker，并以一个裁定（verdict）结束。** fleet 为每个阶段启动一个全新的 Claude Code 会话——评审者不会继承作者的思路。worker 最后用 `afkconvoy verdict` 声明五种裁定之一，这会写下一条**阶段记录（stage record）**：issue 上的一条评论。

| 裁定 | 含义 | issue 去哪 |
|---|---|---|
| `pass` | 这个阶段完成了它的工作 | 去该阶段 `on.pass` 指向的地方：某个阶段、`land` 或 `done` |
| `reject` | 驳回更早阶段的成果 | 回到 `on.reject` 指定的阶段——只能是更早的阶段 |
| `blocked` | 要等别的 issue | 挂起；那些 issue 关闭后同一阶段重跑 |
| `already-satisfied` | 无需改动 | 工作流结束 |
| `escalate` | 这个阶段决定不了 | 阶段指定了会诊角色就先给它，否则给人 |

工作流文件里只写 `pass` 和 `reject`。另外三种在所有工作流的所有阶段含义都相同，所以一个角色只需学一次。

**issue 在哪，是算出来的，不是存下来的。** 每个 tick 读取 issue 的阶段记录和工作流定义，推算出当前阶段。fleet 还会在 issue 上维护一个 `stage:<名称>` 标签，方便你在 GitHub 上筛选——但它从不回读这个标签。手动挪标签没有任何作用；要操纵一张 issue，用命令（[第 7 节](#7-fleet-需要你的时候)）。

**工作流里默认没有人。** 一张 issue 从打上流程标签到进入主干，不需要任何人批准任何事。让这件事安全的是：

- **熔断器（breaker）。** 每次 `reject` 都被计数。`max_rework` 限制每张 issue 的返工总数（默认 3），`max_consecutive_rework` 限制同一个阶段连续驳回的次数（默认 2）。哪一次驳回会越过其中之一，issue 就改为交给人，并列出每一轮。
- **会诊（consult）。** 阶段可以指定一个更资深的角色先问一问——`escalate: { to: attending, do: consult }`。这时它的 `escalate` 会启动一个该角色的 worker，它要么下裁决（原阶段在该裁决下重跑），要么把问题放行给人。这条链只有一个角色长。
- **守卫（guard）。** 绑定仓库的每个 worker 都在 fleet 注入的一个 hook 下运行，它拒绝任何 worker 都不该做的事：合并 PR、push 主干、挪动流程标签、伪造阶段记录。在标记了 `writes: false` 的阶段，它还拒绝 commit 和 push。角色自己的限制则作为 hook 写在角色仓库里。
- **门禁**，每个变更落地前照样要过。

**巡查（patrol）是按时间表运行的角色。** 它不拥有任何 issue。它去找活——没人分流的 issue、坏掉的主干、值得提出的改进——开 issue 并用 `afkconvoy intake` 收进工作流。它不允许提交代码。`max_open` 规定：它开的 issue 还有这么多张未关闭时，就不再运行它。

**机群锁（fleet lock）让一切停下，只做一件修复。** 主干坏了的时候，每张 issue 的门禁都是红的，不管它的 PR 里是什么。一张带着锁标签的未关闭 issue，会让 fleet 只处理锁 issue，直到它关闭。

**先例（precedent）是值得留下的裁决。** 你对一张被升级的 issue 下裁决后，团队指定的那个角色的 worker 会把你的裁决提成团队仓库 `precedents.md` 里的一行，以一个 PR 的形式提交到团队仓库。合并它，此后每个阶段在想为同一个问题升级之前，都会先读到它。

## 5. 完整示例：一家给代码看病的医院

这一节从头到尾搭一整个团队并把它跑起来。下面每条命令、每段输出都来自一次真实运行。它建出的仓库如下（是作者的私有演示仓库）：

| | 仓库 |
|---|---|
| 团队 | [`sunfmin/hospital-team`](https://github.com/sunfmin/hospital-team) |
| 角色 | `sunfmin/hospital-fellow`、`-attending`、`-discharge-nurse`、`-triage-nurse`、`-infection-control`、`-research`、`-safety` |
| 项目 | [`sunfmin/hospital-demo`](https://github.com/sunfmin/hospital-demo) |

这个团队仿照 Cockroach Labs 在 [An experiment in running a hospital for code](https://www.cockroachlabs.com/blog/experiment-running-hospital-code/) 里描述的那个：一张 issue 是一位病人，每个角色只对它做一件事。

| 文章里的角色 | 这里 | 做什么 |
|---|---|---|
| Fellow（专科医师） | 角色 `fellow` | 把 issue 检查清楚、写出方案；在评审过的方案下动手治疗 |
| Review Attending（主治评审） | 角色 `attending` | 评审方案和代码；fellow 决定不了时接受会诊 |
| Discharge Nurse（出院护士） | 角色 `discharge-nurse` | 落地前审计记录，然后执行落地 |
| Triage Nurse（分诊护士） | 角色 `triage-nurse`，作为巡查 | 把每张未分流的 issue 送进合适的工作流 |
| Infection Control（感染控制） | 角色 `infection-control`，作为巡查 | 在主干上跑门禁；坏了就锁住 fleet |
| Research（研究部） | 角色 `research`，作为巡查 | 提出它能证明必要性的工作 |
| Safety（安全部） | 角色 `safety` | 把人的裁决提成先例；复盘工作过程 |
| Charge Nurse（值班护士长） | 不设角色 | fleet 自己会催停滞的 worker、替换死掉的 |
| Chief of Medicine（院长） | 你 | 对升级上来的问题下裁决；合并先例 |

### 第 1 步：创建一个角色

```sh
afkconvoy scaffold role fellow --repo sunfmin/hospital-fellow
```

```json
{"dir":"/…/hospital/fellow","files":["CLAUDE.md",".claude/skills/implement/SKILL.md",".claude/settings.json"],"repo":"sunfmin/hospital-fellow","scaffolded":"role"}
```

它写出一个骨架，提交，并在 GitHub 上创建这个仓库，默认私有（要公开加 `--public`）。大约 7 秒。角色要先于团队创建，因为 `check` 是从 GitHub 读取每个角色的。

### 第 2 步：写这个角色

一个角色由三类文件组成。下面是 fellow，有删节。

**`CLAUDE.md`——无论被要求做什么都遵守的规则：**

```markdown
# Fellow

- **No code before a plan that passed review.**
- **Don't improvise.** When the work needs something the reviewed plan does not hold, stop
  and declare `escalate`, saying what changed.
- **A test is evidence, not an obstacle.** Never delete, skip, loosen or rewrite a test to
  turn it green.
- **Stuck is a finding.** After two attempts at the same failure that did not move it,
  declare `escalate` with a hand-off.
- **Behaviour nobody decided is not yours to decide.** Where the issue leaves open what a
  user of the code would see, read the precedents, and declare `escalate` for what none covers.
```

大意：方案没过评审不写代码；工作超出方案就停下来升级，不即兴发挥；测试是证据不是障碍，绝不为了变绿而删改测试；同一个失败试两次没进展就升级；没人决定过的用户可见行为，不归你决定——先查先例，没有覆盖的就升级。

**`.claude/skills/diagnose/SKILL.md`——它能被要求做的一件事。** 阶段 `workup` 会用 `do: diagnose` 点名它：

```markdown
---
name: diagnose
description: Work an issue up into a treatment plan, changing no file. Use when the brief
  you were started with names `diagnose` as the skill to perform.
---

1. **Read the history.** The issue, every comment, every stage record your brief lists.
2. **Reproduce.** Quote the command and what it printed.
3. **Differential.** The causes that would explain it, and what rules each in or out.
4. **Size it.** Over 1,000 changed lines: split the issue as your brief says.
5. **Find the open questions.** What none of the precedents covers ends the workup: `escalate`.
6. **Write the plan** — Diagnosis, Change, Tests, Risks, Out of scope — as the text of your verdict.
7. **Declare `pass`** with the command your brief gives.
```

技能里从不写「用哪条命令结束这个阶段」：那是 fleet 给每个 worker 写的**简报（brief）**里给出的，issue 和阶段都已填好。

**`.claude/settings.json`——它永远不能做的事**，以 hook 的形式，Claude Code 在每次工具调用前运行：

```json
{ "hooks": { "PreToolUse": [ { "matcher": "Bash",
    "hooks": [{ "type": "command", "command": "\"$CLAUDE_PROJECT_DIR\"/.claude/hooks/refuse.sh" }] } ] } }
```

```sh
#!/bin/sh
call=$(cat)
case "$call" in
  *--no-verify*) echo "fellow: a commit is never made with --no-verify" >&2; exit 2 ;;
  *"git rebase"*) echo "fellow: a branch is never rebased; merge instead" >&2; exit 2 ;;
esac
exit 0
```

退出码 2 表示拒绝这次调用，并把原因显示给 worker。即使 worker 以 `--dangerously-skip-permissions` 运行，hook 依然生效。

提交并 push。**push 到角色仓库的默认分支，是改动到达 worker 的唯一途径。**

其余六个角色用同样的方式创建。要添加别人写好的 skill，在角色的 checkout 里运行 skill 安装器，再把它写出的文件提交：`npx skills add <owner/repo>`。

### 第 3 步：创建团队

```sh
afkconvoy scaffold team team --role fellow=sunfmin/hospital-fellow
```

然后编辑。**`team.md`** 给每个角色起短名：

```yaml
escalate_label: ready-for-human
roles:
  fellow: sunfmin/hospital-fellow
  attending: sunfmin/hospital-attending
  triage-nurse: sunfmin/hospital-triage-nurse
  discharge-nurse: sunfmin/hospital-discharge-nurse
  infection-control: sunfmin/hospital-infection-control
  research: sunfmin/hospital-research
  safety: sunfmin/hospital-safety
precedents: { by: safety, do: record-precedent, max_open: 10 }
```

**`workflows/fix.md`**——用于缺陷，以及任何改变代码行为的变更。「没有方案不写代码，没有评审不算方案」：

```yaml
max_rework: 3
max_consecutive_rework: 2
stages:
  - name: workup
    by: fellow
    do: diagnose
    model: fable
    writes: false
    escalate: { to: attending, do: consult }
    on: { pass: plan-review }
  - name: plan-review
    by: attending
    do: review-plan
    model: opus
    writes: false
    on: { pass: treatment, reject: workup }
  - name: treatment
    by: fellow
    do: treat
    model: opus
    escalate: { to: attending, do: consult }
    on: { pass: code-review }
  - name: code-review
    by: attending
    do: review-code
    model: opus
    writes: false
    on: { pass: discharge, reject: treatment }
  - name: discharge
    by: discharge-nurse
    do: audit
    model: sonnet
    writes: false
    on: { pass: land, reject: treatment }
land: { by: discharge-nurse, do: land }
```

**`workflows/chore.md`**——用于不需要诊断的小改动。第二种任务类型，就是第二个文件：

```yaml
max_rework: 2
stages:
  - name: treatment
    by: fellow
    do: treat
    model: sonnet
    escalate: { to: attending, do: consult }
    on: { pass: code-review }
  - name: code-review
    by: attending
    do: review-code
    model: sonnet
    writes: false
    on: { pass: land, reject: treatment }
land: { by: discharge-nurse, do: land }
```

同样的两个角色，同样的技能，不同的一条线。这就是角色里不写工作流名字的原因。

**`patrols/triage.md`**——一个按时间表运行的角色：

```yaml
by: triage-nurse
do: triage
every: 10m
model: sonnet
max_open: 10
```

### 第 4 步：检查、画出来、预演

```sh
afkconvoy check --dir team
```

它输出读到的内容——每个角色及其技能、每个工作流、每个巡查——或者一次性列出所有错误，每条都带文件和键路径：

```json
{"error":"…","faults":[{"file":"workflows/fix.md","key":"stages[1].on.reject","problem":"…"}]}
```

改完再跑，直到退出码为 0。然后看看你做出了什么：

```sh
afkconvoy show --dir team
```

```
fix — flow:fix — max_rework 3, max_consecutive_rework 2

    ┌─▶ workup       fellow: diagnose · model fable · writes nothing · escalate → attending: consult
    │     ↓ pass → plan-review
    └── plan-review  attending: review-plan · model opus · writes nothing · reject → workup
          ↓ pass → treatment
┌─┬───▶ treatment    fellow: treat · model opus · escalate → attending: consult
│ │       ↓ pass → code-review
│ └──── code-review  attending: review-code · model opus · writes nothing · reject → treatment
│         ↓ pass → discharge
└────── discharge    discharge-nurse: audit · model sonnet · writes nothing · reject → treatment
          ↓ pass → land
        land         discharge-nurse: land
```

再让一张假想的 issue 走一遍。如果 fellow 决定不了，attending 也决定不了，会怎样？

```sh
afkconvoy rehearse --dir team fix escalate escalate ruling pass
```

| 声明了 | 所在阶段 | 之后 issue 处于 |
|---|---|---|
| `escalate` | `workup` | 会诊中：`attending` |
| `escalate` | `workup` | 升级中：等人 |
| `ruling` | `workup` | 再次位于 `workup`，由 `fellow` 执行 |
| `pass` | `workup` | 位于 `plan-review` |

没有启动任何东西，也没有花任何钱。把团队 push 上去：

```sh
cd team && git init && git add -A && git commit -m "The hospital"
gh repo create sunfmin/hospital-team --private --source . --push
```

### 第 5 步：绑定项目

在项目仓库主干上的 `docs/agents/afkconvoy.md` 里：

```yaml
base_branch: main
gate_command: ./gate.sh
concurrency: 3
team: sunfmin/hospital-team
patrols: [triage, rounds]
```

`team` 这个键就是绑定。`patrols` 指明团队的哪些巡查在**这个仓库**运行：团队有四个，这个仓库只跑两个。然后在 checkout 里：

```sh
afkconvoy check                                   # 按这个仓库指向的团队来检查
orca repo add --path "$(pwd)"                     # 每个 checkout 一次：orca 必须认识它
```

第二步很容易漏，而 fleet 目前既不会替你做，也不会替你检查（[#119](https://github.com/sunfmin/afkconvoy/issues/119)）。

### 第 6 步：开 issue

演示项目是一个很小的 Go 包，解析处方里的给药间隔：`ParseInterval("q8h")` 是 8 小时，`DosesPerDay("q8h")` 是 3。三张 issue，用三种不同的方式开：

| # | Issue | 怎么开的 | 为什么 |
|---|---|---|---|
| 1 | `DosesPerDay` 遇到 `"q0h"` 会 panic | 打了 `flow:fix` 标签 | 一个缺陷：由人直接收入工作流 |
| 2 | README：列出支持的间隔写法 | 不打标签 | 留给 triage 巡查去分流 |
| 3 | 支持 `"prn"` 处方 | 打了 `flow:fix` 标签 | issue 里明说有一个答案「尚未决定」 |

开第一个 issue 之前先问 `afkconvoy flows`。它对团队的每个工作流回答：这个工作流是做什么的（工作流文件里 yaml 之上的那段文字）、issue 进入的第一个阶段、以及把 issue 开进这个工作流的那行命令；仓库上缺的流程标签它会顺手建好——否则 `gh issue create --label` 会拒绝一个不存在的标签：

```sh
afkconvoy flows          # 每个仓库一次；团队新增工作流后再跑一次
gh issue create --label 'flow:fix' --title 'DosesPerDay panics on "q0h"' --body-file i1.md
```

```sh
afkconvoy plan
```

```json
{"frontier":[{"number":1,"stage":"workup","title":"DosesPerDay panics on \"q0h\""},
             {"number":3,"stage":"workup","title":"Read \"prn\" prescriptions"}],
 "excluded":[{"number":2,"reason":"no flow label","title":"README: list the interval forms ParseInterval reads"}],
 "free_slots":3,
 "intake":{"limit":50,"open":0},
 "patrols":[{"name":"triage","standing":"due","every":600,"last_run":null,"open":0,"max_open":10},
            {"name":"rounds","standing":"due","every":1800,"last_run":null,"open":0,"max_open":1}]}
```

两张 issue 位于 `workup`，一张没被处理并说明了原因，两个巡查到期待运行。

### 第 7 步：运行

```sh
afkconvoy run --panel 7070
```

fleet 自己打印的行，略有删减：

```
panel: http://127.0.0.1:7070/
tick 2: dispatched #3 · continued #1 · holding 2/3 · frontier 0 · next in 90s
tick 3: advanced #3 · ran the patrol triage, rounds · holding 3/3 · frontier 0 · next in 90s
tick 4: advanced #1 · dispatched #3 · holding 3/3 · frontier 0 · next in 90s
tick 6: advanced #3 · dispatched #1 · holding 3/3 · frontier 1 · next in 90s
tick 7: advanced #1 · dispatched #2 · holding 3/3 · frontier 1 · next in 90s
tick 9: advanced #2 · dispatched #1, #3 · ended the patrol triage, rounds · proposed as a precedent the ruling on #3 · holding 3/3 · frontier 0 · next in 90s
```

`dispatched` 是在某个阶段上启动一个 worker；`advanced` 是 fleet 读到了裁定并结束该阶段的认领（claim）。下一个阶段在后一个 tick 被派发，用它自己的认领。下面是每张 issue 的经历。

#### Issue 2：由巡查分流，走短工作流落地

没人给它打标签。`triage` 巡查的 worker 读了它，读了每个工作流文件里写的「适用于什么」，然后运行了 `afkconvoy intake --issue 2 --workflow chore`。这在 issue 上留下一条**收入记录（intake record）**，并打上了流程标签：

> **afkconvoy** — a worker in the role `triage-nurse`, on the patrol `triage`, took this issue in to the workflow `chore`.

随后 `treatment`（fellow）写好 README 表格并开了 PR #4；`code-review`（attending）通过；落地角色（discharge-nurse）的 worker 对 PR 跑门禁并让它加入落地列车；列车把它落地并关闭了 issue。三个角色，没有人参与。

#### Issue 1：完整走完 `fix`

`workup` 复现了 panic，而且发现的比报告的更多——小时数溢出到恰好为零时同样会 panic：

```
DosesPerDay("q0h")  PANIC: runtime error: integer divide by zero
DosesPerDay("q2251799813685248h") PANIC: runtime error: integer divide by zero   (2^51 hours wraps int64 to 0)
```

它的方案是拒绝「时长为零」，而不是拒绝「小时数为零」，这样一个检查就能堵住两条路。`plan-review` 没有照单全收：它在代码树的副本上亲自跑了复现，确认无误，带着四条备注通过了方案——还有两个输入值得钉住，以及测试应当同时检查错误信息的前缀。`treatment` 照方案实现，先写测试；`code-review` 和 `discharge` 通过；落地。

途中 fellow 发现了一些错误但不会 panic 的输入（`q-8h` 算出每天 -3 次）。它的规则说没人决定过的事不归它决定，方案也已把这些列为范围之外——于是它为它们另开了一张 issue，没打标签。下一轮 `triage` 读了这张 issue，**没有**把它收进工作流：issue 列出了要决定什么，却没有决定本身。按它的规则，它只问一次，留了一条评论——「This can't be worked as written: it lists what to decide but not the decision」——把 issue 留给人来回答。

任何时候都可以问一张 issue 在哪：

```sh
afkconvoy show --issue 1
```

```json
{"position":{"is":"perform","stage":"discharge","by":"discharge-nurse","do":"audit"},
 "follows":{"is":"landing","by":"discharge-nurse","do":"land"},
 "rework":{"had":0,"left":3},
 "records":[{"stage":"workup","verdict":"pass","by":"worker"},
            {"stage":"plan-review","verdict":"pass","by":"worker"},
            {"stage":"treatment","verdict":"pass","by":"worker"},
            {"stage":"code-review","verdict":"pass","by":"worker"}]}
```

#### Issue 3：升级、裁决、成为先例

issue 明说了 `DosesPerDay("prn")` 返回什么尚未决定。fellow 的 `workup` 没有自己挑一个答案。它证明了这个问题躲不开——把 `prn` 读成零时长，会让今天干净的报错变成 panic——然后声明 `escalate`，附上交接说明：问题是什么、三个选项、它自己倾向哪个。

`workup` 指定了会诊角色，所以 fleet 还没有来找人。它启动了一个 `attending` 角色的 worker，执行技能 `consult`。attending 的规则说：调用方看得见、又没人决定过的值，它同样无权裁决。它核对了诊断，补上了 fellow 默认假设了答案的第二个问题，然后放行：

> **afkconvoy: escalated to a human.** The consult of the stage `workup` let its `escalate` through.

这时 issue 上的 `flow:fix` 被换成了 `ready-for-human`。这条链只有一个角色长：fellow、attending、你。

```sh
afkconvoy rule --issue 3 --text 'Ruling, by the Chief:
1. `DosesPerDay("prn")` returns an error, `dose: "prn" has no doses a day`. An interval with no fixed period has no number of doses: say so with an error, never with a number.
2. `ParseInterval("prn")` returns `0, nil`: a zero duration means "no fixed interval".
3. `q0h` is not this issue: #1 decides it.'
```

```json
{"issue":3,"relabelled":true,"ruled":true,"stage":"workup","woke":true,"workflow":"fix"}
```

流程标签回来了，fleet 被唤醒，`workup` 带着简报里的这条裁决重跑。

由于团队指定了谁来提先例，这条裁决还有第二个效果。一个 `safety` 角色的 worker 在[团队仓库开了一个 PR](https://github.com/sunfmin/hospital-team/pull/1)，往 `precedents.md` 里加了一行：

```
- An interval with no fixed period, such as `prn`, reads as a zero duration … and has no number of doses a
  day: say so with an error, never with a number. **When:** a prescription form names no fixed period between
  doses … (https://github.com/sunfmin/hospital-demo/issues/3#issuecomment-…)
```

合并还是关闭，由你决定。合并之后，下一个遇到「没有固定周期的写法」的阶段会读到这条先例，直接照做，不再来问。

### 结果

| | |
|---|---|
| Issue 2，`chore`：2 个阶段加一次落地 | 从开出到落地 18 分钟 |
| Issue 1，`fix`：5 个阶段加一次落地 | 从开出到落地 23 分钟 |
| Issue 3，`fix`：fellow 升级、会诊放行、人裁决一次，然后 5 个阶段加一次落地 | 从开出到落地 28 分钟；落地的代码与裁决一致 |
| 人一共做了什么 | 一次 `afkconvoy rule`；一个先例 PR 留待合并 |
| 人从没做过什么 | 指派、评审、批准、合并 |

发生过的一切都在 issue 上：打开 [issue 1](https://github.com/sunfmin/hospital-demo/issues/1) 从上往下读。每条评论是一个阶段的记录，带着它运行时角色仓库和团队仓库的提交号——所以当某个角色的规则改了，你能分清哪些工作是在哪版规则下做的。

## 6. 运行 fleet

**第一次运行之前**，在仓库的 checkout 里：

```sh
afkconvoy config    # 配置，默认值已填好
afkconvoy check     # 绑定仓库：团队能加载吗？
afkconvoy plan      # 前沿（frontier）：一个 tick 会从哪些开始，其余的为什么在等
```

想知道某张 issue 为什么没被处理，看 `plan`：凡是不在前沿上的未关闭 issue 都列在 `excluded` 下，并带着原因。

**启动。** `run` 永不返回，所以给它一个独立的终端，或者放到后台：

```sh
afkconvoy run --panel 7070
```

它每个 tick 打印一行：

```
tick 1: dispatched #1, #3 · holding 3/3 · frontier 0 · next in 30s
tick 2: nothing to do · holding 3/3 · frontier 0 · next in 60s
```

`holding 3/3` 是当前持有的认领数与 `concurrency` 之比；`frontier` 是在等空位的数量。某一步失败会在这一行里显示为 `FAILED <步骤> #<issue>: <原因>`，循环照常继续。

**观察。** 带上 `--panel <端口>` 后，http://127.0.0.1:端口/ 是一个每个 tick 之后自动刷新的页面：每个工作流画成一串阶段，每个阶段上正在处理和排队的 issue、落地列车、以及在等人的 issue。每个 worker 同时也是 orca 里的一个终端，位于该 issue 的 worktree——打开它就能看到 worker 正在做什么。

**查询一张 issue**：

```sh
afkconvoy show --issue 7
```

它回答 issue 在哪、`pass` 之后会去哪、还剩多少返工额度，以及按顺序排列的每条阶段记录。

**停止**：Ctrl-C，或者杀掉进程。不会释放任何认领，也不会停掉任何 worker：worker 继续工作，同一台机器上下一次 `run` 会把认领接回来。

**要记住的规则**

- 每台机器每个仓库只跑一个 `run`。第二个会抢走第一个的认领。
- worker 在一个全新的登录 shell 里启动，不继承启动 `run` 的那个 shell 的任何东西。默认的 worker 启动命令是 `claude --dangerously-skip-permissions`；如果你是通过包装命令启动 Claude 的，要指明：`afkconvoy run --worker-command <别名>`。
- 对角色或团队的修改，只有 push 到该仓库的默认分支才会到达 worker。一个阶段在启动的那一刻，用的是当时 HEAD 上的版本。
- 对项目配置的修改，进了主干才算数。

## 7. fleet 需要你的时候

只有例外情况才会找你，而且方式始终相同：issue 失去流程标签，换上团队的升级标签（默认 `ready-for-human`），并有一条评论说明原因。所以你只需要看一处：

```sh
gh issue list --label ready-for-human
```

或者面板里的「With a human」部分。afkconvoy 不发任何通知。

| 发生了什么 | 评论里写了什么 | 你做什么 |
|---|---|---|
| 某阶段声明了 `escalate`，且没有会诊裁决它 | 问题本身，通常带选项 | `afkconvoy rule` |
| 熔断器跳闸 | 哪个熔断器，以及每一轮的阶段和原因 | 读每一轮；`rule`，或用 `send` 送去别处 |
| worker 失败了——没动静、没给裁定、门禁红、不该写的阶段留下了提交 | 什么失败了 | 修掉原因；`rule` 让该阶段重跑 |
| issue 在途时工作流被改了 | 某条记录指向一个已不存在的阶段 | 用 `send` 送到一个存在的阶段 |

**在它停下的地方回答**——该阶段重跑，你的文字会出现在它的简报里，两个返工计数都清零：

```sh
afkconvoy rule --issue 7 --text "Keep the v1 schema; migrate in a later issue."
```

**把它放到别处**——放到它工作流的任意阶段，不管记录怎么说：

```sh
afkconvoy send --issue 7 --stage plan --text "The approach changed: see #12."
```

**为属于你的阶段声明裁定**——工作流里写了 `by: human` 的阶段。issue 会无人认领地等在那里，直到你声明：

```sh
afkconvoy verdict pass --issue 7 --stage sign-off --text "Approved."
```

这三条都在项目仓库的 checkout 里运行。不要手动挪标签：那样等于在没有裁决记录的情况下把 issue 交回去，下一个 tick 会再次把它升级。

**先例。** 如果团队指定了谁来提先例，你每次 `rule` 之后，团队仓库都会出现一个往 `precedents.md` 追加一行的 PR。读它的时候问自己：「我是否希望以后每个阶段都不来问、直接照这个做？」——是就合并；如果那条裁决只针对那一张 issue，就关闭。fleet 永远不会在团队仓库上合并任何东西。

**巡查运行失败**时会自己开一张带升级标签的 issue。在你关闭它之前，这个巡查不会再运行。

## 8. 怎样写好角色和工作流

**每样东西放在该放的地方。**

| 它是 | 放在 | 因为 |
|---|---|---|
| 无论被要求做什么都要遵守的规则 | 角色的 `CLAUDE.md` | 该角色执行的每个阶段都会加载它 |
| 怎样做某一件事 | 一个 skill：`.claude/skills/<名称>/SKILL.md` | 阶段的 `do` 点名它 |
| 该角色绝不能做的事 | `.claude/settings.json` 里的 hook | 规则可以被无视；hook 会拒绝 |
| 工作的顺序，以及谁评审谁 | 工作流 | 一个角色服务多个工作流，自己不写任何工作流的名字 |
| 任何变更落地前都必须成立的事 | 项目的门禁 | 角色的 hook 看不到项目自己的规矩 |
| 项目自己的约定 | 项目的 `CLAUDE.md` | 它从 worktree 加载，在每个角色里都生效 |

**角色里不要写阶段名和工作流名。** 技能里最多写到「方案是 workup 最后一次 `pass` 的文字」；fleet 为每个阶段写的简报里已经带了 issue、阶段、worktree、到目前为止的阶段记录，以及结束时要运行的确切命令。技能里写「按你的简报结束」，工作流再怎么改它都还是对的。

**简报说做什么；技能说怎么做。** fleet 的简报是固定格式，完全不涉及怎么干活。一切关于质量的东西——方案要包含什么、什么算阻塞性问题、出院审计查什么——都由你写在技能里。

**让作者和评判者是不同的角色**，并且给每个评判阶段标上 `writes: false`。一个能顺手把问题改掉的评审者，就不再报告问题了。

**给 `reject` 设一个门槛。** 没有「何时该驳回」规则的评审者，会因为一个命名把活打回去。写清什么是阻塞性问题、什么只是备注，以及只有备注就应该 `pass`。熔断器能止住循环；门槛能让循环不开始。

**写清什么不归这个角色决定。** 一个角色里最有用的一句话，就是告诉它什么时候该停下来问。没有这句话，worker 会自己决定产品行为，而你要到看 diff 时才知道。

**在工作流文件里、yaml 上方描述它**：它适用于哪类 issue。负责分流的巡查就是读这段文字来选的。

**运行之前先预演。** `afkconvoy rehearse <工作流> <裁定>…` 让一张假想的 issue 按你列出的裁定走一遍，什么都不启动：

```sh
afkconvoy rehearse fix pass reject pass reject pass reject    # 熔断器在哪一步跳？
afkconvoy rehearse fix escalate ruling pass                   # 会问谁？之后哪个阶段重跑？
```

**从小做起。** 一个角色、一个阶段、能落地，就已经是一个能工作的团队。读过第一个角色的产出之后，再加评审者。

## 9. 参考

### 项目配置——`docs/agents/afkconvoy.md`

| 键 | 默认值 | 含义 |
|---|---|---|
| `base_branch` | 必填 | worker 从它切分支；PR 落到它上面 |
| `gate_command` | 必填 | 只有代码树是好的才退出 0 |
| `concurrency` | `3` | 同时持有的认领数：每个一个 worker |
| `epic_labels` | `[epic, prd]` | 带其中任一标签的 issue 永不处理 |
| `lock_label` | `fleet-lock` | 带它的未关闭 issue 就是一把机群锁 |
| `ready_label` | `ready-for-agent` | 仅未绑定：带着它的 issue 才会被处理 |
| `escalate_label` | `ready-for-human` | 仅未绑定：fleet 放弃一张 issue 时换上它 |
| `team` | — | 团队仓库的 `owner/repo`；写了它就是绑定 |
| `patrols` | `[]` | 仅绑定：团队的哪些巡查在这里运行 |
| `intake_limit` | `50` | 仅绑定：由 worker 收入、尚未关闭的 issue 上限 |

### 团队文件——`team.md`

```yaml
escalate_label: ready-for-human                          # 默认
roles:
  <名称>: owner/repo
precedents: { by: <角色>, do: <技能>, max_open: 10 }      # 可选
```

### 工作流——`workflows/<名称>.md`

在 yaml 之上写清这个工作流是做什么的、它的 issue 该包含什么：`afkconvoy flows` 会把这段文字回答给开 issue 的人，文件里这里什么都没写时 `afkconvoy check` 会给出警告。

```yaml
max_rework: 3                      # 默认
max_consecutive_rework: 2          # 默认
stages:
  - name: <唯一；不能是 land、done、human>
    by: <角色> | human
    do: <技能>                     # 必填，除非 by: human
    model: <模型>                  # 可选；作为 --model 传给 Claude Code
    writes: false                  # 可选；默认 true
    escalate: { to: <角色>, do: <技能> }                         # 可选：会诊
    on: { pass: <阶段> | land | done, reject: <更早的阶段> }      # reject 可选
land: { by: <角色>, do: <技能> }   # 当且仅当有 pass 指向 land 时必填
```

`check` 会拒绝这样的工作流：有阶段到达不了、有阶段走不到终点、`reject` 指向后面的阶段、或者角色或技能不存在。

### 巡查——`patrols/<名称>.md`

```yaml
by: <角色>
do: <技能>
every: 6h          # 一个整数，后跟 m、h 或 d
model: <模型>      # 可选
max_open: 5        # 它开的 issue 还有这么多未关闭时，不再运行
```

### 角色仓库

```
CLAUDE.md                          规则
.claude/skills/<技能>/SKILL.md     每个技能一个目录
.claude/settings.json              hook
```

### 命令，按谁来运行分组

| 谁 | 命令 |
|---|---|
| 操作者 | `config` `check` `plan` `run` `panel` `tick` `takeover` |
| 设计团队的人 | `scaffold` `check` `show` `rehearse` |
| issue 在等的那个人 | `rule` `send` `verdict` |
| worker（简报里会点名每一条） | `gate` `land` `verdict` `intake` `lock` `wake` `rule`（会诊时） |
| worker 的 hook | `guard` |
| 手动执行某一次状态迁移 | `dispatch` `recovery` `close` `escalate` `park` `nudge` `restart` `abandon` |
| 认领的底层管道 | `scan` `claim` `release` `heartbeat` |

每条命令打印一个 JSON 对象；失败时是 `{"error": …}` 并以非零退出。例外：`run` 每个 tick 打印一行，`show` 打印一幅图，`panel` 打印一个页面，`guard` 用退出码回答。

### 绑定仓库的 worker 的环境变量

`AFKCONVOY_ISSUE`、`AFKCONVOY_FLOW`、`AFKCONVOY_STAGE`、`AFKCONVOY_ROLE`、`AFKCONVOY_SKILL`、`AFKCONVOY_WRITES`、`AFKCONVOY_CONSULT`、`AFKCONVOY_BASE_BRANCH`、`AFKCONVOY_ESCALATE_LABEL`、`AFKCONVOY_LOCK_LABEL`、`AFKCONVOY_ROLE_COMMIT`、`AFKCONVOY_TEAM_COMMIT`；巡查的 worker 没有阶段相关的变量，而是有 `AFKCONVOY_PATROL`。角色的 hook 可以读取它们。

### 守卫拒绝什么

对绑定仓库的每个 worker：添加或移除流程标签、`stage:` 标签、升级标签或锁标签；手写阶段记录或收入记录；合并 PR；push 主干。在标记了 `writes: false` 的阶段、对每个会诊、对每个巡查，还包括：commit 和 push。

守卫是靠识别命令工作的。它拦得住犯错的 worker，拦不住存心绕过它的，所以 fleet 事后还会再查一次：不该写的阶段留下了提交，issue 就会被升级。

## 10. 出问题时

先按顺序问三个问题：

```sh
afkconvoy plan --instance <id>     # 前沿上有什么、什么被排除及原因、我持有什么
afkconvoy show --issue <n>         # 某一张 issue 在哪，以及它的记录
afkconvoy takeover --list          # 有哪些 instance id：`run` 从不打印自己的
```

再看 `run` 打印的那些行：某一步失败只会报告在那里，别处没有。

| 你看到的 | 原因 | 怎么办 |
|---|---|---|
| `could not add label: 'flow:fix' not found` | 这个仓库上还没人创建过这个流程标签 | `afkconvoy flows`——仓库上缺的流程标签它都会建好 |
| `FAILED dispatch #1: orca knows no repo at …` | 从没告诉过 orca 这个 checkout | `orca repo add --path <checkout 路径>`；下一个 tick 会自己恢复（[#119](https://github.com/sunfmin/afkconvoy/issues/119)） |
| 某张 issue 没被处理 | 它在 `plan` 的 `excluded` 里，带着原因：没有流程标签、有 epic 标签、有未关闭的阻塞项、有机群锁、被别的实例持有 | 消除原因里说的那件事 |
| `holding 3/3`，前沿不为空，却什么都没发生 | 每个槽位都被占着——巡查的 worker 干完之后，它的那次运行还会占槽大约五分钟（[#122](https://github.com/sunfmin/afkconvoy/issues/122)） | 调大 `concurrency`，或把巡查的 `every` 调长 |
| 上一个阶段结束后，要过一两分钟下一个阶段才启动 | 读到裁定的 tick 和启动下一阶段的 tick 是两个，中间隔着一次休眠（[#123](https://github.com/sunfmin/afkconvoy/issues/123)） | 暂时没办法；这只是延迟，不是故障 |
| `check`、`show`、`rule` 要二十秒 | 每次都把每个角色仓库重新 clone 一遍（[#120](https://github.com/sunfmin/afkconvoy/issues/120)） | 暂时没办法；角色越少越快 |
| 我改了角色，worker 没看到 | 改动没 push 到角色仓库的默认分支，或者那个阶段已经启动了 | push；下一个阶段会在新的 HEAD 下启动 |
| 我把标签挪回去了，它又被升级了 | 标签不是状态：记录里仍然写着「升级中」 | `afkconvoy rule` 或 `afkconvoy send` |
| 某个阶段的裁定「不算数」 | 那条记录写的阶段，不是它的认领被派发到的那个阶段 | `show --issue` 会说明每条记录是怎么被解读的；用 `send` 把 issue 送到该去的地方 |
| issue 因为「一个之前不存在的提交」被升级 | 标记了 `writes: false` 的阶段，或者会诊，提交了代码 | 决定那个提交留不留；`rule` 让该阶段重跑 |
| `the worker launch command … does not take the flags` | 在绑定仓库里，启动命令必须以 Claude Code 结尾 | 指定一个能到达 `claude` 的命令或别名：`--worker-command <别名>` |
| 警告说读不到主干的分支保护 | 免费套餐下的私有仓库不暴露这项信息 | 主干没设保护就没事；否则落地会被拒绝 |
| `run` 被杀掉了 | 什么都没丢 | 在同一台机器上重新启动：它会把认领接回来 |
| 另一台机器的 fleet 挂了 | 它的认领会自己过期 | 等，或者 `afkconvoy takeover --from <对方实例> --instance <你的>` |

目前还没有一条命令能直接说出「正在运行的 fleet 哪里不对、该怎么修」；那是 [#121](https://github.com/sunfmin/afkconvoy/issues/121)。

**想立刻全部停下**：杀掉 `run`，再在 orca 里关掉各个 worker 的终端。issue 上的流程标签和记录都还在，所以之后再 `run`，每张 issue 都会从原来的位置继续。
