# Design — `@文件` 补全

## Context

补全现状：`src/app/autocomplete.rs` 维护斜杠命令候选与菜单状态（打开/选中/滚动/过滤），`overlay` 渲染菜单、`overlay_key` 处理菜单键。pi 侧（`tui/autocomplete.ts`）：`extractAtPrefix()` 从光标前提取 `@token`（含 `@"quoted"` 未闭合形态）→ `getFuzzyFileSuggestions()`：`getBaseDirSuggestions`（readdir 前缀匹配）+ `walkDirectoryWithFd`（`fd` 递归，≤100）合并去重 → `scoreEntry` 打分（exact100/startsWith80/子串50/路径30、目录+10）→ 排序（分数→深度→长度→字典序）→ **top 20** → `buildCompletionValue`（`@` 保留、目录无尾空格、需引号时 `@"…"`）。`fdPath` 为空时 pi 直接无候选。

## Goals / Non-Goals

- **Goals**：token 提取、候选打分排序、应用语义与 pi 对齐；纯本地（readdir + 可选 fd 进程），零上游依赖、零新 RPC
- **Non-Goals**：普通裸路径补全（pi 是 Tab 强制触发的另一条路，后续 change）；`fd` 安装引导；附件上传语义（本项目消息为纯文本，`@path` 以字面量入上下文）

## Decisions

- **D1 提取与候选为纯函数，下沉 `autocomplete.rs`**：
  - `extract_at_token(before_cursor) -> Option<AtToken>`——token 起点=空白/行首/分隔符后的 `@`；支持 `@"…"`（未闭合容忍）；记录 `raw`（含 `@`）与 `query`（引号内/裸路径）
  - `list_at_candidates(query, base_dir, fd_output) -> Vec<AtCandidate>`——目录定位（`~` 展开、绝对、相对、`/` 结尾）、前缀匹配、打分排序、top20 全部纯逻辑；**fd 结果作为可注入参数**（生产由小函数 spawn，单测直接喂样本）
- **D2 fd 缺失回落基础候选**（pi 是空列表）：理由——无 fd 的机器（尤其 CI/最小环境）仍有基础目录可用，体验只升不降；差异记入 `Docs/pi-tui-interaction.md`
- **D3 菜单状态复用现有 completion 菜单**：`CandidateKind::AtFile` 新候选源，同一菜单状态机（打开/导航/Tab/Enter/Esc/重过滤）；Enter 的“应用并提交”分支按候选源区分——文件类应用后走普通提交，斜杠类走命令执行
- **D4 应用逻辑** `apply_at_candidate(text, cursor, candidate) -> (String, usize)`：纯函数——保 `@` 前缀与路径写法、文件→尾空格、目录→无空格、分隔符→自动 `@"…"`、光标=插入点后；单测覆盖六种形态
- **D5 spawn fd 的调用**：`fd -t f -t d --absolute-path <query> <base>`（参数以实现时实际版本验证为准），超时短（如 300ms）静默降级为基础候选；进程失败/超时=无递归候选

## Risks / Trade-offs

- fd 输出格式与版本差异 → 以绝对路径行解析、忽略解析失败行；真实 fd 冒烟放真机验收任务
- Enter 应用并提交对 `@` 语义（字面量入消息）与 pi 附件语义不同——文档记差异，spec 已按字面量定义
