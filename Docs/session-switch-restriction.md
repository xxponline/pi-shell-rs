# 通过扩展限制会话切换

> 归档结论：pi 的扩展机制已原生支持在切换会话时加限制，0 改源码、0 行入侵。

## 结论

使用 `session_before_switch` 事件。pi 在真正切换会话前会先发该事件，扩展 handler 返回 `{ cancel: true }` 即中止切换。

## 关键代码位置

- 事件定义：`packages/coding-agent/src/core/extensions/types.ts`
  - `SessionBeforeSwitchEvent`：`{ type: "session_before_switch", reason: "new" | "resume", targetSessionFile?: string }`
- 事件发射：`packages/coding-agent/src/core/agent-session-runtime.ts:133` 的 `emitBeforeSwitch()`
- 取消语义实现：`packages/coding-agent/src/core/extensions/runner.ts:1109`，handler 返回 `{ cancel: true }` 立即短路，不执行切换。

## 最小扩展示例

放置路径：`~/.pi/agent/extensions/limit-switch.ts`（或项目内 `.pi/extensions/`）。

注册语法以现有扩展样例为准（`export default { handlers: {...} }` 或 `pi.defineExtension(...)`）。

伪代码：

```ts
if (event.type === "session_before_switch") {
    return async (e, ctx) => {
        // 例1：禁止 resume，只允许 new
        if (e.reason === "resume") {
            await ctx.ui.notify({ message: "resume 已被策略禁止", notifyType: "warning" });
            return { cancel: true };
        }
        // 例2：只允许切到特定目录的会话
        if (e.targetSessionFile && !e.targetSessionFile.startsWith("/allowed/dir")) {
            return { cancel: true };
        }
    };
}
```

## 同类可取消事件

| 事件 | 限制什么 |
|---|---|
| `session_before_switch` | new / resume 切换 |
| `session_before_fork` | fork 分支 |
| `session_before_tree` | 会话树内导航（跳到历史节点） |
| `session_before_compact` | 触发 compaction |

机制相同：handler 返回 `{ cancel: true }` 即阻断。
