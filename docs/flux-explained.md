# Flux 在 afkconvoy 里做了什么：十条规则逐行讲解

这篇文章写给没学过 Rust、也没听过 Flux 的读者。读完你应该能看懂 `crates/core` 里那些 `#[spec(...)]` 在说什么，以及为什么要写它们。

文中的代码摘自提交 `d3adb06` 时的 `crates/core/src`。代码以后会改，行号和细节以仓库里的为准。

## 一、这个项目在做什么

afkconvoy 是一个「没人看着也能自己干活」的程序（afk 的意思是「人不在电脑前」）。它做五件事：

1. 去 GitHub 上找待办任务。一个任务叫一个 **issue**。
2. 派一个 AI 工人去做这个任务。
3. 做完后跑一遍检查。这道检查叫 **gate**，意思是「关卡」。
4. 检查通过，才把代码合并进正式版本。正式版本叫 **base 分支**。
5. 做不下去的任务，贴上标签交给人处理。这叫 **升级**（escalate）。

可以同时有好几台电脑在跑这个程序。为了不抢同一个任务，每台电脑要先「认领」：在任务上挂一个牌子，写着「这个我在做」。这个牌子叫 **claim**。

## 二、它怕什么

因为没人看着，出了错没人当场发现。最怕这五种错：

- 两台电脑做同一个任务，互相覆盖。
- 一台电脑死机了，牌子还挂着，任务永远没人做。
- 没通过检查的代码被合并进正式版本。
- 步骤做到一半断电，留下一个半成品，谁也不知道做到哪了。
- 把还有用的工作成果删掉了。

## 三、为什么普通测试不够

平时防错靠「测试」：挑几个例子试一试，结果对就算通过。

问题是例子是人挑的。比如要检查「两个数相减不会得负数」，你试了 10−3 和 8−2 都没问题，但没试 3−10。没试到的情况照样可能出错。

## 四、Flux 是什么

Flux 是一个检查工具。你在代码旁边写下一条规则，比如「这个数永远不会是负数」，它用数学推理判断这条规则在**所有可能的情况**下是否成立。

- 成立：代码放行。
- 不成立：报错，代码不让通过。

测试是「我试了 20 道题都对」，Flux 是「我证明了这个公式永远对」，和数学课上的证明题是一回事。

Flux 只在写代码的阶段跑（`./verify.sh`，也包含在 `./gate.sh` 里）。程序真正运行时不带着它，所以不会变慢。

## 五、先认识几个符号

下面的代码里反复出现这些写法，先看一遍，后面就不重复解释了。

| 写法 | 意思 |
| --- | --- |
| `fn 名字(参数) -> 结果` | 一个函数：给它参数，它算出结果。 |
| `u64`、`usize` | 不能是负数的整数。算出负数会出大错。 |
| `bool` | 只有两个值：`true`（是）和 `false`（否）。 |
| `Option<T>` | 「可能有，也可能没有」。有就是 `Some(值)`，没有就是 `None`。 |
| `&self`、`&mut self` | 「这个东西自己」。带 `mut` 表示函数会修改它。 |
| `#[spec(...)]` | 写给 Flux 看的规则。它不改变程序怎么运行，只说明程序必须满足什么。 |
| `bool[公式]` | 这个 `bool` 的值**恰好等于**公式的结果。 |
| `u64{x: 条件}` | 这个数叫 `x`，并且一定满足条件。 |
| `Self[@s]` | 给「调用前的自己」起个名字叫 `s`，方便后面引用。 |
| `requires 条件` | **调用前**必须成立。不成立，Flux 不让你调用。 |
| `ensures self: Self[...]` | **调用后**自己会变成方括号里描述的样子。 |
| `#[refined_by(a: bool)]` | 给一个结构体挂上 Flux 能看见的标记 `a`。 |
| `#[field(bool[a])]` | 说明结构体里的这个字段，值就是标记 `a`。 |
| `&&`、`\|\|`、`!` | 并且、或者、不是。 |

## 六、十条规则

前五条管「数字」，后五条管「先后顺序」。

---

### 规则 1：怎么判断一台电脑死了

**大白话**

每台电脑定时报一次平安，叫**心跳**。超过 4500 秒（75 分钟）没报，才算死。判得太早，会抢走别人正在做的任务；判得太晚，任务被死机的电脑占着。Flux 保证整个程序用的是同一条标准。

**代码**（`lease.rs`）

```rust
pub const LEASE: u64 = 4500;
pub const SKEW: u64 = 300;

#[spec(fn(stamp: u64, now: u64) -> bool[
    (stamp > now && stamp - now > SKEW) || (stamp <= now && now - stamp > LEASE)])]
pub fn stamp_stale(stamp: u64, now: u64) -> bool {
    if stamp <= now { now - stamp > LEASE } else { stamp - now > SKEW }
}
```

**逐部分解释**

- `LEASE = 4500`：租约长度。心跳在这么多秒之内都算有效。
- `SKEW = 300`：允许两台电脑的钟相差多少秒。
- `stamp`：对方上次报平安的时间。`now`：我现在的时间。
- `-> bool[ ... ]`：返回「是否已死」，而且这个答案恰好等于方括号里的公式。
- `stamp <= now && now - stamp > LEASE`：心跳在过去，并且已经过了 4500 秒以上。这是正常的「太久没消息」。
- `stamp > now && stamp - now > SKEW`：心跳的时间居然在未来，而且超前 300 秒以上。这个时间不可信，也当作死。
- `||`：两种情况有一种成立，就算死。
- 最后一行是真正的计算。Flux 检查它和上面的公式是否完全一致，并且检查两处减法都是「大减小」，不会得负数。

---

### 规则 2：按时报平安的电脑不会被误判

**大白话**

程序每 1500 秒（25 分钟）报一次平安。Flux 证明了：只要按时报，接下来 3000 秒内，谁来看都不会认为它死了。这是最关键的一条，没有它，活着的电脑可能被别人抢走任务。

**代码**（`lease.rs`）

```rust
pub const BEAT_EVERY: u64 = 1500;

#[spec(fn(stamp: u64, now: u64, later: u64)
    requires now <= later && later - now <= LEASE - BEAT_EVERY)]
pub fn a_beat_not_due_outlives_the_wait(stamp: u64, now: u64, later: u64) {
    if !stamp_due(stamp, now) && stamp <= now {
        proved(!stamp_stale(stamp, later));
    }
}

#[spec(fn(bool[true]))]
pub fn proved(_: bool) {}
```

**逐部分解释**

- 这个函数不是给程序用的，它是一道**证明题**。函数名就是题目：「还没到期的心跳，撑得过这段等待」。
- `BEAT_EVERY = 1500`：心跳超过这么老，就该刷新了。
- `later`：未来的某个时刻。
- `requires now <= later && later - now <= LEASE - BEAT_EVERY`：题目的前提。`later` 在 `now` 之后，而且相隔不超过 4500 − 1500 = 3000 秒。
- `if !stamp_due(stamp, now) && stamp <= now`：假设在 `now` 这一刻，心跳还没到该刷新的时候。
- `proved(!stamp_stale(stamp, later))`：那么在 `later` 这一刻，它没有过期。
- `proved` 的规则是 `fn(bool[true])`，意思是「只接受一定为真的值」。如果 Flux 不能证明括号里的东西为真，这一行就报错。所以这一行能通过，就等于题目被证明了。

算一下就明白：心跳最老 1500 秒时还不用刷新，再等最多 3000 秒，一共 4500 秒，刚好没超过租约。

---

### 规则 3：时间差不会算出负数

**大白话**

两台电脑的钟可能差几分钟。对方的钟稍微快一点，时间差按 0 算；快得离谱（超过 5 分钟），就不信这个时间。

**代码**（`lease.rs`）

```rust
#[spec(fn(stamp: u64, now: u64) -> Option<u64{age: age <= now}>)]
pub fn stamp_age(stamp: u64, now: u64) -> Option<u64> {
    if stamp <= now {
        Some(now - stamp)
    } else if stamp - now <= SKEW {
        Some(0)
    } else {
        None
    }
}
```

**逐部分解释**

- 这个函数算「这个时间戳有多老」。
- `-> Option<u64{age: age <= now}>`：结果可能有也可能没有。如果有，这个年龄 `age` 一定不超过 `now`。
- `if stamp <= now { Some(now - stamp) }`：时间戳在过去，年龄就是两者相减。
- `else if stamp - now <= SKEW { Some(0) }`：时间戳在未来，但只超前不到 300 秒，当作「刚刚」，年龄算 0。
- `else { None }`：超前太多，不给答案，表示「这个时间戳不能当证据」。
- 三个分支里的减法都是「大减小」，所以不会出现负数。

---

### 规则 4：同时做的任务数不超过上限

**大白话**

你设定最多同时做几个任务。一轮工作下来，手里的任务数不会超过这个上限。超了会把电脑拖垮，也会多花钱。

**代码**（`tick.rs`，函数体较长，这里只列规则和开头）

```rust
#[spec(fn(set: &WorkingSet, concurrency: usize, held: usize, carry_out: F)
    -> Pass{pass: pass.held <= concurrency || pass.held <= held})]
pub fn run_pass<F>(set: &WorkingSet, concurrency: usize, held: usize, mut carry_out: F) -> Pass
where
    F: FnMut(TickStep) -> Outcome,
```

**逐部分解释**

- `run_pass`：跑一轮工作。程序每隔一段时间跑一轮。
- `set`：这一轮看到的全部情况（哪些任务待做、哪些是我的）。
- `concurrency`：上限，最多同时做几个。
- `held`：这一轮开始时，我手里有几个任务。
- `carry_out`：真正去动手的那个函数。`run_pass` 只做决定，动手交给它。
- `-> Pass{pass: ...}`：返回这一轮的结果 `pass`，并且它一定满足后面的条件。
- `pass.held <= concurrency`：结束时手里的任务不超过上限。
- `|| pass.held <= held`：或者，不比开始时多。

为什么要有「或者」那一半？假设你原来允许同时做 5 个，手里正好 5 个，然后把上限改成 3。这一轮结束时手里可能还是 5 个，超过了 3。但程序保证不会再多拿，只会慢慢减少。

---

### 规则 5：有空位才能接新任务

**大白话**

接任务前必须确认还有空位；还任务前必须确认手里确实有任务。这样「空位数」不会变成负数，规则 4 才站得住。

**代码**（`tick.rs`）

```rust
#[refined_by(held: int, limit: int)]
struct Slots {
    #[field(usize[held])]
    held: usize,
    #[field(usize[limit])]
    limit: usize,
}

impl Slots {
    #[spec(fn(&Slots[@s]) -> bool[s.held < s.limit])]
    fn has_free(&self) -> bool {
        self.held < self.limit
    }

    #[spec(fn(self: &mut Self[@s]) requires s.held < s.limit ensures self: Self[s.held + 1, s.limit])]
    fn take(&mut self) {
        self.held += 1;
    }

    #[spec(fn(self: &mut Self[@s]) requires s.held > 0 ensures self: Self[s.held - 1, s.limit])]
    fn free(&mut self) {
        self.held -= 1;
    }
}
```

**逐部分解释**

- `Slots`：一个记账本，记两个数：`held`（手里有几个）和 `limit`（最多几个）。
- `#[refined_by(held: int, limit: int)]`：让 Flux 也能看见这两个数，后面的规则才能引用它们。
- `has_free`：问「还有空位吗」。它的答案恰好等于 `held < limit`。
- `take`：接一个任务。
  - `requires s.held < s.limit`：调用前必须有空位。
  - `ensures self: Self[s.held + 1, s.limit]`：调用后 `held` 精确加一，`limit` 不变。
- `free`：还一个任务。
  - `requires s.held > 0`：调用前手里必须有任务。
  - `ensures self: Self[s.held - 1, s.limit]`：调用后 `held` 精确减一。

实际效果：程序里如果有人写了「不先问 `has_free` 就直接 `take`」，Flux 证明不了有空位，就会报错。

---

### 规则 6：交给人之前，先写清楚原因

**大白话**

任务做不下去要交给人时，必须先在任务下面留言说明原因，然后才能贴「交给人」的标签。否则人看到一个被退回的任务，却不知道为什么。

**代码**（`transition.rs`）

```rust
#[refined_by(commented: bool, relabelled: bool)]
pub struct Escalation {
    #[field(bool[commented])]
    commented: bool,
    #[field(bool[relabelled])]
    relabelled: bool,
}

impl Escalation {
    #[spec(fn(commented: bool, relabelled: bool) -> Escalation[commented, relabelled])]
    pub fn read(commented: bool, relabelled: bool) -> Escalation {
        Escalation { commented, relabelled }
    }

    #[spec(fn(self: &mut Self[@e]) ensures self: Self[true, e.relabelled])]
    fn comment(&mut self) {
        self.commented = true;
    }

    #[spec(fn(self: &mut Self[@e]) requires e.commented ensures self: Self[e.commented, true])]
    fn relabel(&mut self) {
        self.relabelled = true;
    }
}
```

**逐部分解释**

- `Escalation`：一张进度表，记录一次升级做到哪了。它有两个勾：`commented`（留言了吗）和 `relabelled`（换标签了吗）。
- `read`：从 GitHub 上读回当前进度。两个勾可以是任意组合，因为上一次可能做到一半就断电了。
- `comment`：留言。
  - 没有 `requires`，随时可以做。
  - `ensures self: Self[true, e.relabelled]`：做完后第一个勾变成 `true`，第二个勾保持原样。
- `relabel`：换标签。
  - `requires e.commented`：调用前「留言了」这个勾必须已经打上。
  - `ensures self: Self[e.commented, true]`：做完后第二个勾变成 `true`。

因为进度表是运行时读回来的，所以断电后重新开始，Flux 的检查仍然有效：程序必须先看一眼「留言了吗」，没留就先留，才能换标签。

---

### 规则 7：摘牌子是最后一步

**大白话**

留言、换标签、更新状态板，三件事都做完，才能摘掉「我在做」的牌子。牌子一摘，别的电脑就可能来接手；事情没交代完就摘牌，会留下烂摊子。

**代码**（`transition.rs`）

```rust
pub struct Released {
    issue: u64,
}

#[refined_by(written: bool)]
struct LastWord {
    #[field(bool[written])]
    written: bool,
}

impl LastWord {
    #[spec(fn() -> LastWord[false])]
    fn unwritten() -> LastWord {
        LastWord { written: false }
    }

    #[spec(fn(self: &mut Self[false]) ensures self: Self[true])]
    fn write(&mut self) {
        self.written = true;
    }

    #[spec(fn(&Self[true], issue: u64) -> Released)]
    fn release(&self, issue: u64) -> Released {
        Released { issue }
    }
}

impl Escalation {
    #[spec(fn(Self{e: e.commented && e.relabelled}, word: &LastWord[true], issue: u64) -> Released)]
    fn release(self, word: &LastWord, issue: u64) -> Released {
        word.release(issue)
    }
}
```

**逐部分解释**

- `Released`：一张「已摘牌」的回执。整个文件里只有 `release` 能造出它。
- `LastWord`：状态板的「最后一句话」，只有一个勾：`written`（写了吗）。状态板是给人看进度的地方。
- `unwritten()`：新建一个还没写的。`LastWord[false]` 表示勾是空的。
- `write`：写上最后一句。`Self[false]` 要求调用前没写过，`ensures self: Self[true]` 说明调用后写了。所以它只能写一次。
- `LastWord::release`：`&Self[true]` 要求状态板已经写过，才给回执。
- `Escalation::release`：摘牌子。它的参数有三个要求：
  - `Self{e: e.commented && e.relabelled}`：进度表上两个勾都打了。
  - `word: &LastWord[true]`：状态板已经写了最后一句。
  - 三样齐全，才返回 `Released`。

整个升级函数 `escalate` 的返回值是 `Released`。想让这个函数「成功结束」，就必须拿到回执；想拿到回执，就必须三件事都做完。半途而废的升级没有办法冒充已完成。

---

### 规则 8：任务没关闭，不能删工作文件夹

**大白话**

每个任务有自己的工作文件夹，叫 **worktree**。只有任务确认关闭了，才能删它。没关闭的任务，文件夹里可能还有没保存到别处的成果，删了就没了。

**代码**（`settle.rs`）

```rust
#[refined_by(closed: bool, dropped: bool, removed: bool, boarded: bool)]
struct SettleOrder {
    #[field(bool[closed])]
    closed: bool,
    #[field(bool[dropped])]
    dropped: bool,
    #[field(bool[removed])]
    removed: bool,
    #[field(bool[boarded])]
    boarded: bool,
    steps: Vec<SettleStep>,
}

impl SettleOrder {
    #[spec(fn(self: &mut Self[@o]) requires o.closed && !o.removed ensures self: Self[o.closed, true, o.removed, o.boarded])]
    fn drop_branches(&mut self) {
        self.steps.push(SettleStep::DropBranches);
        self.dropped = true;
    }

    #[spec(fn(self: &mut Self[@o]) requires o.closed && o.dropped ensures self: Self[o.closed, o.dropped, true, o.boarded])]
    fn remove_worktree(&mut self) {
        self.steps.push(SettleStep::RemoveWorktree);
        self.removed = true;
    }

    #[spec(fn(self: &mut Self[@o]) requires o.closed && o.removed && o.boarded)]
    fn release(&mut self) {
        self.steps.push(SettleStep::Release);
    }
}

pub fn settle_steps(closed: bool) -> Vec<SettleStep> {
    let mut order = SettleOrder::new(closed);
    if !order.closed {
        order.close_issue();
    }
    order.close_pr();
    order.drop_branches();
    order.remove_worktree();
    order.follow_base();
    order.board();
    order.release();
    order.steps
}
```

**逐部分解释**

- `SettleOrder`：任务做完后「收尾」的进度表，有四个勾：
  - `closed`：任务关闭了吗。
  - `dropped`：任务的分支清理了吗。
  - `removed`：工作文件夹删了吗。
  - `boarded`：状态板更新了吗。
- `steps`：一张清单，按顺序记下要做的步骤。每个函数往清单里加一步（`push`），同时打上对应的勾。
- `drop_branches`：清理分支。要求任务已关闭（`o.closed`），并且文件夹还没删（`!o.removed`），因为要靠文件夹确认哪些分支属于这个任务。
- `remove_worktree`：删文件夹。要求任务已关闭（`o.closed`）并且分支已清理（`o.dropped`）。
- `release`：摘牌子。要求任务已关闭、文件夹已删、状态板已更新，和规则 7 一样排在最后。
- `settle_steps`：把上面这些按顺序串起来。如果有人把其中两行换个位置，比如把 `remove_worktree` 挪到 `close_issue` 前面，Flux 会报错，因为前提不成立。

---

### 规则 9：没过关卡的代码不能进正式版本

**大白话**

要合并代码，必须拿出一张「通行证」。通行证只在三件事都对得上时才发：是这份代码、是这道关卡、检查结果还没过期。这样就不能拿别的代码的通过记录，或者很久以前的通过记录蒙混过关。

**代码**（`gate.rs`）

```rust
defs! {
    fn fresh(at: int, now: int) -> bool {
        (at <= now && now - at <= RECORD_TTL) || (at > now && at - now <= SKEW)
    }
}

#[refined_by(tree: int, command: int, at: int, now: int)]
#[invariant(fresh(at, now))]
pub struct Proven {
    name: String,
    #[field(u64[tree])]
    tree: u64,
    #[field(u64[command])]
    command: u64,
    #[field(u64[at])]
    at: u64,
    #[field(u64[now])]
    now: u64,
}

impl Proven {
    #[spec(fn(tree: &Tree[@t], command: &GateCommand[@c], run: &GreenRun, now: u64)
        -> Option<Proven{p: p.tree == t && p.command == c && p.now == now}>)]
    pub fn by(tree: &Tree, command: &GateCommand, run: &GreenRun, now: u64) -> Option<Proven> {
        if run.tree == tree.key && run.command == command.key && run.stands(now) {
            Some(Proven {
                name: tree.name.clone(),
                tree: run.tree,
                command: run.command,
                at: run.at,
                now,
            })
        } else {
            None
        }
    }
}
```

**逐部分解释**

- `tree`：一份代码的完整内容。每份内容有一个唯一的编号，内容变一点，编号就变。
- `command`：关卡跑的是哪条检查命令。
- `GreenRun`：一条「检查通过」的记录，记着是哪份代码、哪条命令、什么时候（`at`）通过的。
- `fresh(at, now)`：「这条记录还新鲜吗」的定义。在过去且不超过 `RECORD_TTL`（代码注释里说是一天），或者稍微超前但不超过 `SKEW`。
- `Proven`：通行证。
  - `#[invariant(fresh(at, now))]`：任何一张通行证都必须满足「记录是新鲜的」。造不出一张过期的通行证。
- `Proven::by`：发通行证的唯一窗口。
  - `-> Option<Proven{...}>`：可能发，也可能不发。
  - `p.tree == t`：发出来的通行证，代码编号一定是你问的这份。
  - `p.command == c`：检查命令一定是你问的这条。
  - `p.now == now`：是按现在这个时刻判断的。
- 函数体里的 `if`：记录的代码对得上、命令对得上、而且还没过期（`run.stands(now)`），三样都满足才 `Some(...)`，否则 `None`。

把代码推到正式版本的那个函数要求传入一张 `Proven`。拿不到通行证，那个函数就调用不了。

---

### 规则 10：合并的必须是大家看得到的代码

**大白话**

合并前，代码必须先上传到 GitHub。否则合并进去的是只存在于某台电脑上的代码，别人在网页上从没见过，也没法审查。

这里的「火车」（train）是一条排队合并的线：通过的任务一个接一个接到火车上，整列火车过了关卡再一起进正式版本。

**代码**（`train.rs`）

```rust
#[refined_by(pushed: bool, merged: bool)]
struct JoinOrder {
    #[field(bool[pushed])]
    pushed: bool,
    #[field(bool[merged])]
    merged: bool,
    steps: Vec<JoinStep>,
}

impl JoinOrder {
    #[spec(fn(pushed: bool) -> JoinOrder[pushed, false])]
    fn new(pushed: bool) -> JoinOrder {
        JoinOrder { pushed, merged: false, steps: Vec::new() }
    }

    #[spec(fn(self: &mut Self[@o]) ensures self: Self[true, o.merged])]
    fn push_branch(&mut self) {
        self.steps.push(JoinStep::PushBranch);
        self.pushed = true;
    }

    #[spec(fn(self: &mut Self[@o]) requires o.pushed ensures self: Self[o.pushed, true])]
    fn merge(&mut self) {
        self.steps.push(JoinStep::Merge);
        self.merged = true;
    }

    #[spec(fn(self: &mut Self[@o]) requires o.merged)]
    fn push_line(&mut self) {
        self.steps.push(JoinStep::PushLine);
    }
}

pub fn join_steps(pushed: bool) -> Vec<JoinStep> {
    let mut order = JoinOrder::new(pushed);
    if !order.pushed {
        order.push_branch();
    }
    order.merge();
    order.push_line();
    order.steps
}
```

**逐部分解释**

- `JoinOrder`：「上火车」的进度表，两个勾：`pushed`（上传了吗）和 `merged`（接到火车上了吗）。
- `new(pushed)`：新建进度表。`pushed` 由外面告诉它（可能之前已经上传过），`merged` 一开始是 `false`。
- `push_branch`：把任务的代码上传到 GitHub。做完后 `pushed` 变成 `true`。
- `merge`：把代码接到火车上。`requires o.pushed` 要求已经上传。
- `push_line`：把火车本身上传。`requires o.merged` 要求已经接上了东西，不会上传一列空火车。
- `join_steps`：按顺序排步骤。注意 `if !order.pushed { order.push_branch(); }` 这一行：
  - 如果没上传，就先上传，勾变成 `true`。
  - 如果已经上传，勾本来就是 `true`。
  - 两种情况走完，Flux 都能确定 `pushed` 是 `true`，所以下一行 `merge()` 才被允许。
  - 如果有人删掉这个 `if`，Flux 就无法确定已上传，`merge()` 那行会报错。

---

## 七、怎么知道规则本身没写错

规则也可能写得太松，什么都放行。所以项目里还专门写了几十段「故意做错」的代码，前面标着 `#[should_fail]`，意思是「这段必须被 Flux 拦下来」。例如 `transition.rs` 里的这一段：

```rust
/// An issue relabelled with no reason on it.
#[should_fail]
fn relabelled_before_the_comment() {
    let mut begun = Escalation::read(false, false);
    begun.relabel();
}
```

它读回一张两个勾都是空的进度表，然后直接换标签。按规则 6，这不允许。如果哪天有人把规则 6 的 `requires e.commented` 删了，这段代码就不再报错，而 `#[should_fail]` 会让 Flux 反过来报「该错的没错」。这样规则被削弱时会被立刻发现。

另外 `gate.sh` 里有一步检查：`crates/core/src` 里不许出现 `trusted`、`ignore`、`opaque` 这几个标记。它们的作用是「这段不用检查，相信我」。禁止它们，就没有哪段代码能绕过 Flux。

## 八、Flux 管不到的地方

- **只管「做决定」的代码。** `crates/core` 只负责决定下一步做什么。真正去 GitHub 读数据、写数据的是 `crates/cli`，那部分不归 Flux 管。如果 GitHub 返回的信息本身是错的，Flux 不知道。
- **只管「调用前必须满足什么」。** 它能保证「摘牌子之前三件事都做完了」，但不能保证「程序一定会走到摘牌子那一步」。后者靠规则 7 里说的回执机制，由普通的 Rust 代码保证。
- **有额外的成本。** 需要安装 `cargo-flux`、`fixpoint`、`z3` 几个工具，要用 Rust 的 nightly 版本，规则要人一条条手写。

设计上的取舍记录在 [ADR-0007](adr/0007-refinement-types-on-a-pure-core.md)。
