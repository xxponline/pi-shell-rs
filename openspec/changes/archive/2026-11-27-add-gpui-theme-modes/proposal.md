# Proposal

## Why

pi 主系统（1.6.0）已支持 light / dark / system 三模式主题，且主项目将持久化
该 mode 并在嵌入场景下经 gpui 全局 `Theme::change` 注入；而我们的 GPUI 独立
窗口仍钉死浅色（`pin_light_theme` + 手调浅色调色板），规格中也明确把「完整
主题系统」列为后续工作。作为模块交付时，窗口自身必须具备三模式渲染能力，
否则宿主设了 mode 我们也不会画。

## What Changes

- GPUI 独立窗口主题 mode 三态：`light | dark | system`，**默认 `system`**
  （跟随 OS 外观，live 热切换）；mode 为窗口内存态，**不持久化、不回写
  pi `settings.json`**（持久化属主项目职责）。
- 双调色板（P1，pi 同源）：`LIGHT_PALETTE` 硬编码升级为按 mode 解析的
  两套 token 表——语义 token（工具卡三态、通知、bash/exit、滚动条、文字
  角色等）取自 pi `light.json` / `dark.json`（okhsl 预计算为 RGB 常数），
  面板底色类（消息区/状态行/补全行）维持自有值：浅色现值不动、深色补一套。
  工具卡随模式：浅色模式下浅卡深字（替换现「白面板 + 深卡」混血），深色
  模式深卡浅字（现值）。
- 选择入口：`/settings` 浮层新增 Theme 行，循环 `system → light → dark`
  （doubleEsc 同款交互，但只改内存、即时生效、不写文件）。
- 输入区边框随模式：浅色保持纯黑 1px/2px（现值），深色新增浅色系边框。
- 规格：`gpui-agent-terminal` 两条需求变更（浅色固定 → 三模式；输入边框
  补深色变体）。

## Capabilities

### New Capabilities

（无）

### Modified Capabilities

- `gpui-agent-terminal`：`独立窗口浅色模式固定` 更名并改写为独立窗口主题
  三模式（默认 system、内存态不落盘、双调色板 pi 同源、工具卡随模式、
  system 订阅 OS 外观）；`输入区简约边框` 补深色模式边框措辞。

## Impact

- 代码：`src/gpui_agent/terminal.rs`（palette 模块 → 双表按 mode 解析、
  ~33 处消费点）、`src/gpui_agent/window.rs`（去 `pin_light_theme`、默认
  system + 外观订阅）、`src/gpui_agent/overlay.rs` 或共享浮层（/settings
  Theme 行，仅内存态）。
- 值源：pi `dist/modes/interactive/theme/{light,dark}.json`（预计算，不引
  运行时解析器）。
- 不动：`src/app/*`（TUI/embed 注入机制零改动）、`tui-app` 规格、pi RPC、
  pi `settings.json`（不读不写 `theme` 键）。
- 文档：`Docs/tui-usage.md` GPUI 节（浅色固定 → 三模式选择与已知边界）。
