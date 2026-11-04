# probe 使用说明

驱动 `pi --mode rpc`、抓取真实 JSONL 事件流并归档成 fixture 的命令行工具。
实现背景与协议细节见 [`rpc-protocol-notes.md`](./rpc-protocol-notes.md)。

## 前置条件

1. 已安装 `pi`（PATH 里能找到，本机实测 v1.5.0），且已用原生 pi 登录过
   （凭据在 `~/.pi/agent/auth.json`）——探针不做交互式登录。
2. 在本仓库根目录编译：

```bash
cargo build --bin probe
# 产物：target/debug/probe
```

> 如果 `pi` 不在 PATH，或想指定别的可执行文件：`export PI_BIN=/path/to/pi`。
> 探针实际 spawn 的是 `<pi> --mode rpc`（不是旧文档里的 `pi-agent`）。

## 常用命令

### 查看内置场景

```bash
./target/debug/probe --list
```

### 跑单个场景

```bash
./target/debug/probe --scenario text_only
```

除 `extension_ui` 外的场景都会真实调用一次模型，产生少量 token 费用。

### 跑全部场景

`extension_ui` 依赖 fixture 扩展，跑全部时用 `--ext` 带上：

```bash
./target/debug/probe --all --ext probe-fixture-extension/probe-ui.ts
```

单独跑 extension_ui：

```bash
./target/debug/probe --scenario extension_ui \
  --ext probe-fixture-extension/probe-ui.ts
```

## 选项

| 选项 | 作用 |
|---|---|
| `--list` | 列出内置场景后退出 |
| `--scenario <name>` | 只跑指定场景 |
| `--all` | 依次跑全部场景；单个失败不中断后续，最后统一报告 |
| `--out <dir>` | 产物输出根目录，默认 `probe-out/` |
| `--timeout-ms <ms>` | 每步等待超时，默认 60000，模型慢可调大 |
| `--snapshot` | 额外生成截断后、纳入 git 的快照到 `tests/fixtures/probe/<场景>/snapshot/` |
| `--ext <path>` | 透传为 `pi -e <path>` 加载扩展，可重复传多个 |
| `-h, --help` | 帮助 |

## 内置场景

| 名称 | 抓什么 |
|---|---|
| `text_only` | 纯文本对话的完整生命周期 + text delta |
| `tool_call` | 触发 read 工具，抓 `toolcall_*` 与 `tool_execution_*` 交织顺序 |
| `thinking` | thinking 级别设为 high，抓 `thinking_*` delta |
| `steer` | 生成中发 steer，观察打断与 `queue_update` |
| `follow_up` | 生成中排队两条 follow_up，抓多 turn 与 `queue_update` |
| `compact` | 关闭自动压缩后手动 compact，抓 `compaction_start/end` |
| `failed_tool` | 读不存在的文件，抓 `tool_execution_end.isError=true` |
| `extension_ui` | 经 fixture 扩展触发 select/confirm/notify 弹窗 |

## 产物布局

```
probe-out/
└── <场景名>/
    ├── runs/<时间戳>/        # 每次运行的完整原始产物（git 已忽略）
    │   ├── transcript.jsonl  # 逐行 {seq, ts_ms, dir, raw}，raw 为原始消息
    │   ├── stderr.log        # pi 的 stderr（带时间戳，正常应为空）
    │   ├── summary.json      # 事件分类计数 + 每步成败 + failure
    │   └── meta.json         # pi 版本、git sha、cwd、binary 等
    └── latest/               # 最新一次运行的副本（不用看时间戳）
```

加 `--snapshot` 时另写：

```
tests/fixtures/probe/<场景名>/snapshot/   # 纳入 git，供离线测试回放
├── transcript.jsonl   # 超长字段（>4000 字符）被截断
└── summary.json       # truncations 记录被截断的路径与原始长度
```

## 怎么看抓到的事件

先看 `summary.json`：`counts` 是各类事件出现次数，`steps` 是每步成败，
`failure` 是失败原因。要逐条看时间线，读 `transcript.jsonl`：

```bash
python3 -c "
import json, glob
d = sorted(glob.glob('probe-out/text_only/runs/*/'))[-1]
for line in open(d + 'transcript.jsonl'):
    r = json.loads(line); raw = r['raw']
    sub = ''
    if raw.get('type') == 'message_update':
        sub = '.' + raw['assistantMessageEvent']['type']
    if raw.get('type') == 'response':
        sub = '.' + raw.get('command', '')
    print(r['seq'], r['dir'], r['ts_ms'], raw.get('type') + sub)
"
```

## 退出码

- `0`：选中场景全部成功
- `1`：有场景失败（`--all` 下不中断后续场景，末尾报告 `N/total failed`）
- `2`：参数错误 / 场景名不存在

失败时产物仍会落盘，看对应 run 的 `summary.json` 定位卡住的步骤。

## 注意事项

- 场景会**真实调用模型并产生少量费用**，prompt 都刻意写得很小。
- `compact` 预期收到 `"Nothing to compact (session too small)"` 业务错误——
  压缩阈值是 20000 token，探针会话达不到；但 `compaction_start/end` 仍会发出，
  探针用 `expect_error` 容忍该错误，场景算成功。
- 尚未覆盖：Rust 主动发 `bash`（验证 `bash_execution_update`）、resume 会话
  （验证 `entry_appended`）、其余扩展 UI method
  （input/editor/setStatus/setWidget/setTitle/set_editor_text）。
  加场景改 `src/bin/probe/scenarios/mod.rs`。
