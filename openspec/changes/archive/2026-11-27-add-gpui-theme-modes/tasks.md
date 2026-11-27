# Tasks

## 1. 调色板双表（值源与结构，design D2/D6）

- [x] 1.1 按 pi `light.json`/`dark.json` 同算法预计算 okhsl → sRGB，产出
      **31 字段**双表并落成 `Palette` 结构 ×2 常数（映射：pi 同源 **23 个**
      ——`BASH_BODY←muted`、`BASH_CMD/WARN/CARD_STATUS_RUN←warning 系`、
      `CARD_DIM←dim`、`CARD_TEXT←toolTitle`、`TOOL_PREVIEW_TEXT←toolOutput`、
      `CARD_STATUS_OK/EXIT_OK←success`、`CARD_STATUS_ERR/EXIT_BAD/NOTICE_ERROR←error`、
      `NOTICE_RETRY←warning`、`NOTICE_DEFAULT←muted`、`THINKING_TEXT←thinkingText`、
      `USER_LABEL←accent`、`CARD_FG_RED/GREEN←toolDiffRemoved/Added`、
      `CARD_FG_YELLOW←warning`、`CARD_FG_GRAY←syntaxComment`、
      `CARD_BG_PENDING/OK/ERR←toolPendingBg/toolSuccessBg/toolErrorBg`（工具卡
      **面板底**入表——混血根源，随模式取色的前提）；自有 **8 个**
      ——`CHAT_BG/STATUS_BG/INPUT_BORDER/COMP_*`（浅色=现值、深色=手调；
      `INPUT_BORDER` 深色按 4.1 取 pi dark border）、`CARD_FG_CYAN`（pi 无青
      token、`syntaxType` 实为紫——修正起草期映射；浅 #0097A7 手调、深
      #6EC6C6 现值）；验证：单测断言两表齐全、深色工具卡三态与
      `TuiTheme::default`（pi dark 同源）一致、浅色三态为浅值
      ——已落成：pi-tui 同算法生成器校验 5/5 命中既有 dark 现值；31 字段
      `Palette` + `LIGHT`/`DARK` + `for_dark` 落库；palette 单测 5/5、
      列 B 497 全绿、clippy 双列 0（过渡期 dead_code allow 注明）
- [x] 1.2 `palette` 模块消费点改造：33 处 `palette::X` 改为按 resolved
      mode 取表（`Palette::for_mode`），面板/文字/卡片/bash/通知全部走表；
      验证：列 B `cargo test --features gpui-support` 既有几何/行为测试全绿
      （颜色断言按新双表更新）
      ——已落成：render 入口解析 `Theme::global(cx).mode` 一次；纯函数链
      （entry→assistant/thinking/tool_card/notice/bash→segs→seg_visual/
      fg_on_card）穿 `&'static Palette`；旧单表常数删除；Fg diff 测试改
      双表新值；列 B 498、clippy 双列 0
- [x] 1.3 工具卡随模式取色：浅色模式浅卡深字（替换白面板+深卡混血）、
      深色模式深卡浅字；验证：单测断言 light/dark 两模式下卡片底与卡上
      文字色分别取自各自表
      ——已落成：面板底抽 `card_panel_bg(status,is_error,pal)` 纯函数
      （tool_card 调用），6 断言覆盖两模式×三态；卡上文字经 seg_visual
      随 pal 取表
- [x] 1.4 滚动条 token 按 mode 覆写为 pi 值（spec: 消息区滚动条 pi 同源；
      explore 对账新增）：经 kit `ThemeConfig` 三字段
      （`scrollbar.background` / `scrollbar.thumb.background` /
      `scrollbar.thumb.hover.background`）在 light/dark 两配置写入 pi
      `scrollbarTrack/Thumb` 预计算值，`Theme::change` 后内建滚动条随 mode
      取 pi 色；验证：测试断言两 mode 下 track/thumb 解析值 = pi 预计算
      RGB（dark #484E52/#97A0A5）、hover 行为不回归
      ——已落成：`window.rs::apply_pi_scrollbar_tokens` 补丁两份
      `ThemeConfig.colors` 三字段（track/thumb/hover；kit 默认主题显式给了
      hover 灰值，必须覆盖否则残留）后 `Theme::change` 重解析；测试断言
      浅 #e1e3e4/#96a0a4、深 #484e52/#97a0a5、hover=thumb、切回 mode
      补丁仍在；列 B 499、clippy 双 0

## 2. mode 状态与 system 跟随（design D1/D3/D5）

- [x] 2.1 内存态 mode（`Light|Dark|System`，默认 `System`）与纯解析函数
      `resolved(mode, appearance) -> Light|Dark`；验证：单测覆盖三 mode ×
      两 appearance 的解析矩阵与默认值
      ——已落成：新建 `gpui_agent/theme.rs`（`ThemeSetting` 默认 System +
      `resolved` 四外观矩阵 + `ThemeState` Global 读写）；3 测试绿（含
      set/get 往返与未初始化回退）
- [x] 2.2 system 订阅接线：切到 system 时经窗口外观观察通路
      （`appearance_observers` / `set_window_appearance` 注入）把 OS 外观
      变化送到 `Theme::change(resolved)`，切离后不再动作（幂等）；验证：
      测试注入 appearance 翻转断言全局 `Theme.mode` 跟随、切 light 后注入
      不再改
      ——已落成：`apply`/`on_appearance_changed(_with 可注入决策体)`/
      `init_theme` 三入口；run() 换 `init_theme`（删 pin_light_theme 及
      其测试）+ `observe_window_appearance` 订阅保活于
      `AppearanceSubscription` Global；测试：System 跟随注入翻转、固定
      态注入不动作、启动默认解析
- [x] 2.3 重绘 spike：mode 变化后本组件一帧内按新色重绘；若 kit 不通知，
      补 `cx.observe_global::<Theme>` → `window.notify()` 兜底；验证：测试
      切换后渲染帧关键色为新值（一帧断言）
      ——spike 结论：kit `Theme::edit` 末尾自带 `cx.refresh_windows()`
      （theme/mod.rs），真实窗口由其驱动自动出帧，**无需** observe_global
      兜底；测试以 `cfg(test)` 帧色探针（LAST_RENDER_CHAT_BG）断言 Light
      帧 0xFFFFFF / Dark 帧 0x1E1E1E 一帧切换

## 3. /settings 选择行（design D4）

- [x] 3.1 `/settings` 浮层新增 Theme 行：显示当前 mode、循环
      `system → light → dark`、即时生效；**不写任何文件**（与 doubleEsc
      回写的唯一差别）；验证：e2e 断言三态循环、选择后 `settings.json`
      内容不变（前后字节比对）
      ——已落成（全链路）：共享 `SurfaceKind`（默认 Tui）+
      `Overlay::Settings.theme_row`（open_settings 按表面置位）→
      `overlay_key` 夹取 2/3 门控 → `apply_settings` idx3 发
      `UiEvent::ThemeCycle`（TUI 空臂不可达）→ GPUI 泵 `cycle_and_apply`
      （内存态循环，A3 零写盘）→ GPUI 浮层 Theme 行显示 `ThemeState` 值；
      测试 3 枚：游标门控（TUI 恒 2）、共享事件+文件字节不变+TUI 对照、
      GPUI e2e（真实 slash 开 /settings→Down×3→Enter×3 循环
      system→light→dark→system、mode/帧色探针随表、settings.json 字节
      全程不变、浮层保持）
- [x] 3.2 选择热生效链路：mode 改 → kit 内建控件与我们的 token 同帧切换；
      验证：测试切 dark 后消息区底、输入边框、工具卡均为深色值
      ——已落成：三支帧色探针（thread_local 按测试线程隔离——进程级
      static 曾被并行测试互踩）；`switching_to_dark_repaints_panels_border_and_card`
      断言三面浅(#FFFFFF/#000000/#DEE9E1)→深(#1E1E1E/#5FA8CC/#254131)

## 4. 输入边框双模式（spec: 输入区简约边框 MODIFIED）

- [x] 4.1 输入边框随 mode：浅色纯黑 1px/2px（现值不变）、深色浅色系
      （pi dark 同源预计算值）；验证：测试断言两模式边框色与聚焦加粗
      不变
      ——已落成：两模式边框色由 palette 双表承载（浅 #000000/深 #5FA8CC，
      `switching_to_dark` + `panel_surfaces_per_mode` 断言）；聚焦加粗抽
      `input_border_width(focused)` 纯规则、render 消费同一真源，
      真值表测试钉住 1px/2px 不变

## 5. 文档

- [x] 5.1 `Docs/tui-usage.md` GPUI 节：「浅色固定」改写为三模式（默认
      system、`/settings` 循环选择、不落盘重启回默认、浅色浅卡为 pi 同源
      新基准、auto/自定义主题名不解析为已知边界）；验证：文档与实现一致、
      照做可用

      ——已落成：tui-usage GPUI 节主题 bullet 全量改写（三态/默认 system/
      /settings theme 行不落盘/双调色板 pi 同源/浅卡新基准/滚动条 pi 值/
      auto与自定义不解析、syntax 两模式同用的已知边界）+ 输入边框 bullet
      补深色变体；与实现逐项一致
## 6. 集成验收

- [x] 6.1 双矩阵全绿：`cargo test` + `cargo clippy --all-targets`（列 A
      共享零回归）与 `cargo test --features gpui-support` + clippy（列 B）
      零告警；验证：命令输出全绿、clippy 0
      ——A 417+10+3 / B 510+10+3 全绿，clippy 双列 0
- [x] 6.2 `openspec validate add-gpui-theme-modes --strict` 通过；验证：
      命令输出 valid
      ——`openspec validate add-gpui-theme-modes --strict` valid
- [x] 6.3 PTY 人工冒烟：默认 system 跟随 macOS 外观翻转、`/settings` 三态
      切换热生效且 `settings.json` 未被写、浅色浅卡 / 深色全套对比度、
      深色输入边框可辨、重启回 system；验证：用户实机验收通过
      ——用户验收通过（2026-11-27 实机窗口冒烟）
## Workflow follow-up

- 实现完成后 `/opsx-archive add-gpui-theme-modes`（含规格同步：
  REMOVED+ADDED+MODIFIED → `gpui-agent-terminal` 主规格）。
