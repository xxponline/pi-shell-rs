# Tasks

## 1. 依赖与 session 状态扩展

- [x] 1.1 `cargo add pulldown-cmark syntect`（syntect 关闭默认主题全集、仅保留所需深色主题的 feature 以控体积）；验证 `cargo build` 通过且记录首次编译耗时与二进制体积
- [x] 1.2 `Usage` 增加 cost 字段（input/output/cache_read/cache_write/total，f64），`from_json` 读 `cost.*` 缺失为 0；验证单测：含 cost 解析正确、缺 cost 不失败、token 计数不变
- [x] 1.3 `auto_retry_start/end` 归约为 `Notice{kind:"retry"}`（含 attempt/max/错误摘要与最终结果）；验证单测：两条通知依次出现且 phase/消息不受影响
- [x] 1.4 新增 `SessionState::apply_get_state(&Value)`：写入 model/sessionName/thinkingLevel，缺失字段保留旧值；验证单测：应用与缺字段两个场景
- [x] 1.5 核对已实现的"运行级错误 → error 通知"与 spec 场景逐条对应（errorMessage 落成/turn_end 不重复/正常消息无通知）；验证既有 3 个测试的断言覆盖 spec 场景，缺口补齐

## 2. markdown 渲染管线

- [x] 2.1 新建 `src/app/markdown.rs`：pulldown-cmark 事件流 → `Vec<Line>`，覆盖标题/粗体/斜体/行内代码/列表/引用/链接（链接显示文本+目标提示）；验证纯函数单测断言各元素样式与文本内容
- [x] 2.2 代码块渲染：围栏边框/缩进 + 语言标签 + 未闭合围栏临时闭合（防流式闪烁）；验证单测：闭合与未闭合两种输入均产出完整代码块且不 panic
- [x] 2.3 syntect 语法高亮接入：按语言高亮代码行，内容哈希缓存（含容量上限），固定深色主题；验证单测：rust 代码块着色 span 数 >1、同内容二次渲染命中缓存、未知语言降级为纯文本
- [x] 2.4 畸形与极端输入测试：未闭合嵌套、10KB 长文、无闭合粗体、二进制样字符；验证全部不 panic 且渲染终止
- [x] 2.5 性能冒烟：10KB markdown 连续渲染 100 帧耗时在合理阈值内（断言 < 2s，验证缓存生效）
- [x] 2.6 chat.rs 接入：assistant/user 的 Text 块经 markdown 渲染，thinking/tool/notice 走原路径，user 输入保持原文不解析；验证既有 fixture 回放测试更新后全绿

## 3. thinking 折叠

- [x] 3.1 `chat_lines(state, flags)` 增加 `RenderFlags { show_thinking, expand_tools }`，App 维护 `show_thinking`（默认折叠），`ctrl+t` 切换；验证键位逻辑单测（ctrl+t 取反、默认 false）
- [x] 3.2 thinking 渲染两态：折叠=单行占位、展开=完整文本弱化样式；验证单测：两种 flags 下同一状态的渲染断言

## 4. 工具卡片（先抓 fixture）

- [x] 4.1 probe 新增 `tool_bash` 场景（prompt 触发 bash echo 类小命令）与 `tool_edit` 场景（prompt 触发对临时文件的 edit，跑完恢复原状）；用 `--snapshot` 归档两个 fixture；验证 `tests/fixtures/probe/{tool_bash,tool_edit}/snapshot/` 存在且离线回放测试能加载
- [x] 4.2 新建 `src/app/tools.rs` 卡片框架：头行（工具名·关键参数·状态·耗时）、失败红色、通用兜底卡（未知工具名+args 摘要）、结果防御式读取（content/structuredContent/details 缺失均降级）；验证单测：成功/失败/未知工具三种卡片
- [x] 4.3 read 卡片：路径 + 内容预览（structuredContent 优先，content[].text 次之）；验证 tool_call fixture 回放断言含路径与内容片段
- [x] 4.4 bash 卡片：命令摘要 + 输出（截断时提示 fullOutputPath）；验证 tool_bash fixture 回放断言含命令与输出片段
- [x] 4.5 edit 卡片：`details.diff` 结构化红绿优先，缺失时对 args old/new 做行级简易 LCS 对比降级；验证 tool_edit fixture 回放断言 diff 行红绿存在，且仅有 args 时降级路径可测
- [x] 4.6 `ctrl+o` 全局展开/折叠（默认折叠为头行+结果首 3 行）；验证 flags 单测与展开态渲染断言

## 5. 状态行补全

- [x] 5.1 状态行格式化函数：model 名、tokens（in↑/out↓/cache，千分位缩写如 1.2k）、cost（`0→不显示`、`<0.01→$0.004`、`≥1→$1.23`）；验证三个 cost 边界 + tokens 缩写的纯函数单测
- [x] 5.2 接线：preflight 的 get_state 响应调用 `apply_get_state`，header 渲染 model/cost，流式中随 usage 刷新；验证 PTY 手工确认状态行数值变化 + 单测覆盖 header 渲染字段缺失不 panic

## 6. 队列 UI 与键位

- [x] 6.1 输入框上方队列带：steering/follow_up 分别标注前缀、空队列不占行、超出 2 行截断显示 `…N more`；验证空/单条/超限三种渲染单测
- [x] 6.2 键位：`alt+enter` 发送 follow_up（清空输入、流式不打断）、`alt+up` 弹出 follow_up 队尾回填输入（空队列无副作用）；验证键位映射单测（含无 ALT 的 enter/up 行为不变）
- [x] 6.3 retry 通知黄色渲染（区分 error 红），error 红保持；验证两种 Notice 样式断言

## 7. 验收与收尾

- [x] 7.1 真机 PTY 手工验收清单并与原生 pi 并排对比：markdown（标题/粗体/代码块着色）、thinking 折叠切换、read/bash/edit 卡片与 ctrl+o、状态行 tokens·cost·model、alt+enter 排队与 alt+up 取回、retry/error 提示；验证逐项截图/目测确认"基本无差别"，差异记入 tui-usage 已知差异
- [x] 7.2 更新 `Docs/tui-usage.md`（新键位、界面区域、已知差异）并在 `Docs/roadmap.md` 阶段 3 标注完成；验证文档命令/键位与实现一致
- [x] 7.3 全量 `cargo test`（含新 fixture 回放）与 `cargo clippy --all-targets` 零告警；`openspec validate align-chat-experience --strict` 通过；验证三项命令输出
