# Tasks

## 1. TuiTheme 与 chat_lines_themed（先行提交）

- [x] 1.1 新增 `TuiTheme` 结构（5 颜色字段 + syntect 主题名，`Default` = 现
      RGB 值与 `"base16-ocean.dark"`），放在渲染链可达的模块；`tool_panel_bg`
      改为读 `theme` 字段——验证：`cargo test` 中
      `tool_card_panel_bg_three_states` 与 `chat.rs:904-906` 精确 RGB 断言
      **零改动全绿**
- [x] 1.2 新增 `chat_lines_themed(..., theme)`，原 `chat_lines` 薄包装转发
      `TuiTheme::default()`；`append_blocks`/`tool_panel_bg` 吃 theme——验证：
      `rg "chat_lines\(" src/` 确认存量 28 处调用点一行未改，
      `cargo test` 全绿
- [ ] 1.3 为注入路径补单测：`chat_lines_themed` 传非默认 `TuiTheme` 后三态
      面板色等于注入值、传默认等于现值——验证：新测试通过且现有测试零改动
- [ ] 1.4 第一次 commit（`cargo test` 全绿 + `cargo clippy --all-targets`
      零告警后提交）

## 2. 滚动条色参数化

- [ ] 2.1 `wrap_for_viewport` 加 `theme` 参数，滚动条 track/thumb 从
      `TuiTheme` 取值；`const` 保留为默认值定义（`view.rs` 测试引用点不改）；
      `refresh_selection_doc` 路径传 Default——验证：`cargo test` 全绿、
      `view.rs` 滚动条断言零改动通过
- [ ] 2.2 补滚动条注入测试：非默认 theme 下 track/thumb 格子 fg 等于注入值
      ——验证：新测试通过

## 3. markdown 主题可切换与双缓存清理

- [ ] 3.1 `theme()` 改为 `OnceLock<ThemeSet>` 按名查表；新增
      `render_markdown_themed(text, theme)` + 默认包装；非法主题名回退默认
      且调用方可感知（返回值表达，不 panic）——验证：`cargo test` 全绿，
      新增非法名回退测试通过
- [ ] 3.2 新增两缓存清理入口（`HL_CACHE` + `MD_CACHE` 同清）与
      `highlight_by_path`/`push_code_block` 的 themed 下传——验证：新增
      "切主题后渲染不含旧主题色" 测试（先用旧主题渲染、切新主题、断言
      代码块 fg 为新值）通过
- [ ] 3.3 commit

## 4. embed 注入与运行时切换

- [ ] 4.1 `EmbedConfig` 加 `theme: Option<TuiTheme>`（None=默认），spawn 时
      应用；`EmbeddedApp::set_theme` 经 keys 通道送 `UiEvent::Theme`——验证：
      embed 单测断言注入值送达与 None 默认路径行为不变
- [ ] 4.2 `UiEvent::Theme` + event loop arm：应用 `self.theme`、清双缓存、
      `need_draw = true`；`App` 持 theme 并填入 `ViewUi`——验证：
      `TestBackend` 无头测试断言 `set_theme` 后一帧内工具卡面板/滚动条/
      代码高亮三注入面全部按新值出色
- [ ] 4.3 commit

## 5. spec 验收与文档

- [ ] 5.1 对照 `specs/tui-app/spec.md` delta 的 6 个 scenario 逐条核验
      （默认字节不变 / 注入生效 / 一帧切换 / 缓存不漏 / 非法名回退）——
      验证：`cargo test` 全绿覆盖前四条，非法名回退测试覆盖第五条
- [ ] 5.2 `Docs/theme-injection-requirements.md` 验收项销账标注；交付说明
      补一句宿主 ANSI 调色板对比度为宿主职责——验证：文档更新且与实现一致
- [ ] 5.3 最终全量检查：`cargo test` + `cargo clippy --all-targets` 零告警；
      `cargo run` 手工确认 standalone 外观与今天无差别——验证：命令输出干净

## Workflow follow-up

- 归档 change（`/opsx-archive`），delta 合入 `openspec/specs/tui-app/spec.md`。
