# Proposal

## Why

输入区光标位置按**字符序号**直接当**终端格列**用(`view.rs`: `cx = area.x + 1 + cur_col`),
宽字符(CJK/emoji,显示宽度 2)会让光标左偏:输入 `你好😊hahah` 时字符序号为 8 →
光标画在第 9 格,而按显示宽度应在第 12 格 — 光标落在文本中间。纯 ASCII 输入时
字符数==格数,缺陷不可见,故长期未被测试覆盖(现有 view 测试全用 ASCII)。
水平滚动偏移 `col_off` 与可见行截取存在同样的字符/显示列混淆。

## What Changes

- `app/view.rs` 输入区光标定位改为按**显示列**计算:光标行前缀按
  `unicode-width` 求格数,`cx = area.x + 1 + 前缀显示宽 - 滚动偏移`。
- 水平滚动 `col_off` 改为显示列偏移;可见行按显示列截取,新增辅助函数
  (跳过/截取不切断宽字符,跨偏移点的宽字符整字保留)。
- 编辑器 `EditorBuffer` 不动(字符序号语义是编辑层正确语义,只修渲染换算)。
- 新增 view 单测:宽字符行光标显示列、宽字符行水平滚动。

## Capabilities

### New Capabilities

_(无)_

### Modified Capabilities

- `tui-app`: 新增需求"输入光标显示定位" — 输入区终端光标 MUST 按显示列
  定位(全角/emoji 计 2 格),水平滚动与可见截取 MUST 按显示列计算且不切断
  宽字符;附宽字符光标位置与宽字符水平滚动两个场景。

## Impact

- **代码**: 仅 `src/app/view.rs`(输入区渲染 + 其 `#[cfg(test)]` 模块)。
- **依赖**: 无新增(`unicode-width` 已是依赖,`wrap.rs` 在用)。
- **行为**: 纯 ASCII 输入渲染与光标位置完全不变;含宽字符时光标回到正确格列。
- **下游**: balabala-agent-gui 通过路径依赖直接受益(其截图诊断即本缺陷)。
