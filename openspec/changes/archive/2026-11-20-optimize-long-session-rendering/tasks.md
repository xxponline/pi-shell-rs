# Tasks

> 阶段划分沿 `Docs/transcript-incremental-rendering.md` §5：P1 = 组 1–4，
> P2 = 组 5，P3 不在本变更（design.md Non-Goals，触发条件见文末）。
> 每组落地自己所需的测试；提交前 `cargo test` + `cargo clippy --all-targets`
> 全绿（AGENTS.md 强制），函数注释中文三要素。

## 1. 变更检测（design D7，P1 前置）

- [x] 1.1 `SessionState` 增 `rev: u64` / `dirty_from: usize` 与 `take_dirty()`
  （纯 usize，不引入层依赖）；单测断言读取即复位、无变异时返回"无脏"
  （`cargo test` 全绿）。
- [x] 1.2 变异点埋点（`transcript.rs` 约 12 处：push 系 / `apply_tool` /
  `apply_tool_result` / `flush_inflight` / `load_messages` / bash 系），
  逐类单测断言 `dirty_from` 取被触最小条目下标、全量替换取 0、流式与 bash
  尾字段记**尾部脏**（`dirty == len`，不落条目下标——对账非空、重排范围
  为空；选项 A 修订）（`cargo test` 全绿）。
- [x] 1.3 `handle()` 出口统一 `rev++` 安全网 + draw 帧尾 rev 对账（不一致 →
  全量重建，debug_assert 提示）；单测模拟漏标场景验证降级为全量重建而非
  stale 渲染。

## 2. 条目级布局缓存（design D1/D2/D9，P1 核心）

- [x] 2.1 新建 `src/app/layout_cache.rs`：`EntryLayout`（含
  `wrapped_rows_per_line`）、`LayoutCache`、`LayoutKey`、两个易失尾槽
  （inflight + bash，每帧重建，不入缓存 Vec）；条目内局部
  thinking/msg 计数 + 基数组合（读取时重定基）。模块单测断言局部→全局编号
  与现全量路径逐号相等。
- [x] 2.2 `chat_lines_themed` / `render_chat` 接缓存：按 `take_dirty` 增量
  重排版 `dirty_from..` + 尾部窗口 K≥2，前缀每帧全量重算（整数加法），
  `LayoutKey` 变化整体重建；接线后 2.3 parity 单测先行合入（与本任务同组）。
- [x] 2.3 **Parity 单测（硬性验收）**：同一 `SessionState` 输入下缓存路径 vs
  全量路径逐字节相等（`lines` / `meta` / `logical_start` / `thinking_seq` /
  `msg_seq` / `wrap_for_viewport` 输出 / `collect_hits` 区间 / `ChatScroll`
  回写）；覆盖空会话 / 单条目 / 流式中途 / override toggle / 宽度变化 /
  中部变异（`apply_tool` 乱序命中倒数第 2 条）全谱（`cargo test` 全绿）。

## 3. state_slot 删除（design D8，BREAKING）

- [x] 3.1 删除 `state_slot` 字段、`draw` 开头的每帧 `SessionState` 深拷贝
  发布、`EmbeddedApp::session_state()` 口与 embed 侧 Arc 接线；全仓
  `grep state_slot|session_state` 零残留，`cargo test` + `cargo clippy
  --all-targets` 全绿（宿主仓零引用已核实，编译零破坏）。

## 4. 基准与集成验收（design D0/D6，横跨组 1–3）

- [x] 4.1 基准脚本化：以 `replay_jsonl` 回放合成基准会话（约 1500 条目 /
  ≥5MB，脚本合成、**不含真实会话内容**）计时 `chat_lines + wrap`，宽松阈值
  断言 < 5ms/帧（防 CI 抖动）；同步实测全量 `EntryLayout` 缓存内存值并回填
  design.md D6 校准注记（`cargo test` 或 `cargo bench` 一键可跑）。
  注记（实测，`cargo test --lib bench_warm_frame -- --nocapture`）：合成
  1500 条目 / 5.8MB → 暖帧 best=**1.06ms**（refresh 0.67 + wrap 0.39）< 5ms ✓；
  冷建 585ms（允许一次 O(全文)）；旧全量路径 600ms（**566×**）；缓存足迹
  **19.2MB**（< 36MB 触发线，D6 已回填实测）；完整 draw（TestBackend）2.60ms。
- [x] 4.2 长会话 PTY 手工冒烟（按 `Docs/tui-usage.md`）：打开基准会话滚动 /
  打字 / 选区 / override 切换，观察无卡死、滚动绝对行号与贴底判定与现状一致；
  结果记录在本任务勾选注记中。
  注记（PTY 实测 2026-11-20；100×40 PTY，cwd=balabala 项目，一次性驱动
  脚本于 /tmp）：
  - 基准会话确认：`/resume` 首位 = 7.4MB 会话文件；pi `get_messages` 实测
    398KB / **1829 条消息**（60 用户 / 781 助手 / 967 工具）——pi 侧即以
    紧凑消息提供（非文件原文），TUI 加载的就是这份长会话。
  - 启动/切换：启动首帧正常；切换加载（含全量冷建）末输出 ≈1.2s。
  - 滚轮 30 事件（30/s）连发：帧间隔 avg **9.7–13.5ms**、连发期内无任何
    内部停顿、帧持续到达、无卡死（曾观测到的“1.02s 间隙”已定位：= 连发
    结束后的正常静默 + `SCROLLBAR_HIDE_DELAY=1s` 滚动条瞬态隐藏重绘，
    非卡顿）。
  - 键盘回显：PageUp/Down **3.3–4.2ms**；打字 **4.6ms**（输出为增量 diff，
    全文字节断言不适用，回显有响应）；ctrl+u 4.2ms。
  - override 切换：ctrl+t（LayoutKey 整体重建）回显 **18–31ms**——与 1829 条
    会话的全量重建规模成比例（合成基准 full_rebuild=591ms@5.8MB 参照）。
  - 选区：鼠标拖选（按下→拖→抬起）响应 **10–33ms**，有重绘输出。
  - 滚动绝对行号/贴底判定：PTY 字节流无法逐行核验，等价性由 2.3/5.2
    parity 单测逐字节覆盖；交互面观察 = 滚动帧随位置变化、回到底部正常、
    全程无异常崩溃。
  - 退出：干净退出 code 0。结论：**长会话交互无卡死 ✓**。

## 5. 视口化 wrap（design D3，P2）

- [x] 5.1 `wrap.rs` 新增"按 wrap 计数前缀和切逻辑行区间 `[l0,l1)`"入口，
  只对可见段调 `split_on_newlines` + `wrap_one`，bg 补齐只做窗口内；
  全文路径保留供选区；模块单测覆盖窗口边界（空行段/宽行/贴底）。
- [x] 5.2 回归：窗口输出与全文路径切片逐字节相等（含滚动条几何与 hit
  区间），`selection_doc` 行数口径（wrap 后屏行）与 `content_len` 判据不变
  （`cargo test` 全绿）。
- [x] 5.3 基准复测：4.1 脚本重跑，`chat_lines + wrap` 合计 < 5ms 达标，
  滚轮 30 事件/s 输入回显实测 < 50ms（本地计时注记）。
  注记（实测）：复跑同基准 → 暖帧 **1.06ms** < 5ms ✓；回显预算 = 事件处理
  （µs 级滚动算术）+ 完整 draw **2.60ms**（TestBackend 实测）≪ 50ms；真实
  宿主链路交互回显并入 4.2 PTY 冒烟观察。

## Workflow follow-up（归档相关，非跟踪任务）

- 归档时同步修订 `Docs/transcript-incremental-rendering.md`：机制归因
  （死亡螺旋→每帧 O(全文)）、热路径补漏（bg-pad 第三遍、`state.clone()`）、
  变更检测决策（D7）、验收指标（帧率/延迟口径）。
- 归档注记写明 `EmbeddedApp::session_state()` 删除（BREAKING）并在
  CHANGELOG 留一句。

## 可选扩展（触发后另立变更，本变更不做）

- 冷首帧分帧（首帧只排前 N 屏 + 后台补全）——触发条件：4.2 冒烟实测冷首帧
  明显可感。
- D6 内存上限档 2（近窗口 ±500 条 + LRU）/ 档 3（meta 二级缓存）——触发
  条件：4.1 实测缓存内存 > 估算预算 2 倍。
