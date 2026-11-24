//! 核心薄层：kameo RPC 路由 actor（design D3）。
//!
//! 职责边界（spec: 核心薄层 RPC 路由）：拥有 `RpcClient` 的命令通道，
//! 交互命令以请求-应答（ask）转发并按 id 匹配响应，会话事件以通知（tell）
//! 扇出给全部订阅方；**不做状态归约**——归约由各前端自带的
//! [`crate::session::SessionState`] 完成。
//!
//! 生命周期（design D3 实施期细化）：[`RpcSession`] 持有 `RpcClient`
//! （shutdown / Drop 回收子进程，幂等），actor 持其 `CommandSender` 子集；
//! 事件由独立泵线程从 `take_events()` 读出后 tell 进 actor 扇出。

use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use kameo::actor::{ActorRef, Spawn};
use kameo::error::SendError;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use serde_json::Value;

use crate::rpc::{ClientError, ClientEvent, CommandSender, ResponsePolicy, RpcClient};

/// 交互命令请求（spec: 核心薄层 RPC 路由——ask 形态）。
#[derive(Debug, Clone)]
pub struct CommandRequest {
    /// 待转发的命令 JSON（如 `{"type":"prompt",...}`），id 由泵线程注入。
    pub value: Value,
    /// 等待响应的超时；超时返回 [`ClientError::Timeout`]。
    pub timeout: Duration,
    /// 成功判定策略（`RequireSuccess` / `AcceptError`）。
    pub policy: ResponsePolicy,
}

/// 登记一个事件订阅方（spec: 事件扇出——支持多订阅前端）。
pub struct Subscribe {
    /// 订阅方的事件接收端；actor 扇出时发送失败即视为失效并剔除。
    pub tx: Sender<ClientEvent>,
}

/// kameo 薄层路由 actor：命令交 `RpcClient` 泵线程按 id 路由，
/// 事件由泵线程 tell 进本 actor 后扇出给全部订阅方；不做状态归约。
#[derive(kameo::Actor)]
pub struct RpcActor {
    /// `RpcClient` 的命令句柄（阻塞的 `send_command` 在独立线程执行，
    /// 经 `DelegatedReply` 稍后回执，不拖住事件扇出——design D3 细化）。
    command_sender: CommandSender,
    /// 订阅方接收端列表（扇出时发送失败即剔除）。
    subscribers: Vec<Sender<ClientEvent>>,
}

impl Message<CommandRequest> for RpcActor {
    type Reply = DelegatedReply<Result<Value, ClientError>>;

    /// 把命令交给独立线程执行阻塞的 `send_command`，立即归还应答权。
    ///
    /// 输入: msg — 命令 JSON + 超时 + 成功策略；
    ///       ctx — 取得 `DelegatedReply` 标记与一次性回执句柄。
    /// 输出: `DelegatedReply` 标记（ask 方稍后收到 `Result<Value, ClientError>`）；
    ///       无回执句柄（tell 形态）时命令仍执行、结果丢弃。
    async fn handle(
        &mut self,
        msg: CommandRequest,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (delegated, reply_sender) = ctx.reply_sender();
        let sender = self.command_sender.clone();
        thread::spawn(move || {
            let result = sender.send_command(msg.value, msg.timeout, msg.policy);
            if let Some(reply) = reply_sender {
                reply.send(result);
            }
        });
        delegated
    }
}

impl Message<Subscribe> for RpcActor {
    type Reply = ();

    /// 把订阅方接收端登记进扇出列表。
    ///
    /// 输入: sub — 携带订阅方 `Sender` 的登记消息；
    /// 输出: 无返回值；副作用为列表追加一项（后续事件起参与扇出）。
    async fn handle(&mut self, sub: Subscribe, _: &mut Context<Self, Self::Reply>) {
        self.subscribers.push(sub.tx);
    }
}

impl Message<ClientEvent> for RpcActor {
    type Reply = ();

    /// 把一条会话事件扇出给全部存活订阅方，失效接收端就地剔除。
    ///
    /// 输入: event — 无 id 的会话事件 / 解析错误 / 关闭通知；
    /// 输出: 无返回值；副作用为逐订阅方投递 + 剔除断开的订阅方。
    async fn handle(&mut self, event: ClientEvent, _: &mut Context<Self, Self::Reply>) {
        self.subscribers
            .retain(|tx| tx.send(event.clone()).is_ok());
    }
}

/// 进程级共享 tokio 运行时（kameo actor 的调度宿主；多线程、自驱动，
/// 首次使用时惰性创建，进程生命周期内存活）。
///
/// 功能: 提供 `Handle::current()` 供 `RpcActor::spawn` 落任务；
/// 输入: 无；
/// 输出: `'static` 运行时引用（构建失败即 panic，无运行时则 actor 无法工作）。
fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("无法创建 tokio 运行时")
    })
}

/// 事件泵：把 `RpcClient` 的事件流逐条 tell 进 actor 扇出。
///
/// 输入: events — `RpcClient::take_events()` 的接收端（泵线程独占）；
///       actor — 路由 actor 引用（持有其强引用直至泵退出）。
/// 输出: 无返回值；副作用为事件逐条转发；`Closed` 转发后退出，
///       通道断开（显式 shutdown 路径未发 `Closed`）则补发一次 `Closed`
///       让订阅方感知会话结束；actor 已停则直接退出。
fn pump_events(events: Receiver<ClientEvent>, actor: ActorRef<RpcActor>) {
    let mut closed_forwarded = false;
    while let Ok(event) = events.recv() {
        if matches!(event, ClientEvent::Closed) {
            closed_forwarded = true;
        }
        if actor.tell(event).blocking_send().is_err() {
            return;
        }
        if closed_forwarded {
            return;
        }
    }
    // 通道断开且未转发过 Closed：补发一次（spec: 事件扇出/关闭感知）。
    let _ = actor.tell(ClientEvent::Closed).blocking_send();
}

/// 会话句柄：拥有 `RpcClient`（关闭权与子进程回收）+ 路由 actor + 主事件订阅。
///
/// 主订阅在事件泵启动**之前**建立， spawn 与订阅之间无事件丢失窗口；
/// 额外订阅方经 [`RpcSession::subscribe`] 登记。
pub struct RpcSession {
    /// 路由 actor 引用（ask 命令 / tell 登记订阅）。
    actor: ActorRef<RpcActor>,
    /// RPC 客户端（shutdown 幂等；Drop 时自动回收 pi 子进程）。
    client: RpcClient,
    /// 主事件订阅（spawn 时即建立，零丢失窗口）；`take_events` 后为 None。
    events: Option<Receiver<ClientEvent>>,
    /// `get_state` 探活的原始返回（核心不做归约，交前端 apply）。
    pub probe: Value,
}

impl RpcSession {
    /// 拉起 `pi --mode rpc`、探活并建立路由 actor 与事件泵。
    ///
    /// 功能: spawn 子进程 → `get_state` 探活（15s 超时）→ 建 actor/主订阅/泵线程；
    /// 输入: binary — pi 可执行文件路径；extra_args — 透传参数（如 `-e <ext>`）；
    ///       cwd — 子进程工作目录（`None` 继承）；
    /// 输出: 成功返回 [`RpcSession`]（`probe` 字段为探活原始数据）；
    ///       失败返回中文可读 `Err`（spawn 失败 / 探活失败，且已杀子进程）。
    /// 须在任意线程调用（内部自建共享运行时，不依赖调用方 tokio 上下文）。
    pub fn spawn(
        binary: &str,
        extra_args: &[String],
        cwd: Option<&Path>,
    ) -> Result<RpcSession, String> {
        Self::spawn_with_probe_timeout(binary, extra_args, cwd, Duration::from_secs(15))
    }

    /// [`RpcSession::spawn`] 的探活超时可调形态（测试用短超时）。
    ///
    /// 功能: 同 `spawn`，探活截止期由 `probe_timeout` 控制；
    /// 输入: 同 `spawn` + probe_timeout — `get_state` 的等待上限；
    /// 输出: 同 `spawn`。
    fn spawn_with_probe_timeout(
        binary: &str,
        extra_args: &[String],
        cwd: Option<&Path>,
        probe_timeout: Duration,
    ) -> Result<RpcSession, String> {
        let mut client = RpcClient::spawn_in(binary, extra_args, cwd)
            .map_err(|e| format!("无法启动 pi ({binary}): {e}"))?;
        let probe = match client.send_command(
            serde_json::json!({"type": "get_state"}),
            probe_timeout,
            ResponsePolicy::RequireSuccess,
        ) {
            Ok(data) => data,
            Err(e) => {
                // 探活失败：杀子进程后返回可读错误（spec: 启动探活与错误提示）。
                client.shutdown();
                return Err(format!("pi 探活失败 (get_state): {e}"));
            }
        };

        let events = client.take_events();
        let command_sender = client.command_sender();
        // 主订阅先于事件泵建立：spawn→前端订阅之间零事件丢失。
        let (primary_tx, primary_rx) = mpsc::channel();
        let actor = {
            let _guard = runtime().enter();
            RpcActor::spawn(RpcActor {
                command_sender,
                subscribers: vec![primary_tx],
            })
        };
        let pump_actor = actor.clone();
        thread::spawn(move || pump_events(events, pump_actor));

        Ok(RpcSession {
            actor,
            client,
            events: Some(primary_rx),
            probe,
        })
    }

    /// 主事件订阅只读访问（未被组件接管时可用；已接管则 panic——同
    /// `RpcClient::events` 口径）。
    ///
    /// 功能: 返回主订阅接收端引用；
    /// 输入: 无；
    /// 输出: 事件接收端引用（`take_events` 后 panic）。
    pub fn events(&self) -> &Receiver<ClientEvent> {
        self.events.as_ref().expect("events already taken")
    }

    /// 取走主事件订阅接收端（组件接管事件泵；再次调用 panic——同
    /// `RpcClient::take_events` 口径）。
    ///
    /// 功能: 所有权移交调用方（通常是 GPUIAgentTerminal 的归约泵）；
    /// 输入: 无（`&mut self`）；
    /// 输出: 主订阅接收端；副作用为内部槽位置空（后续 `events()` panic）。
    pub fn take_events(&mut self) -> Receiver<ClientEvent> {
        self.events.take().expect("events already taken")
    }

    /// 经路由 actor 发送一条交互命令并等待其响应。
    ///
    /// 功能: ask 形态——命令按 id 路由、超时与成功策略由载荷指定；
    /// 输入: value — 命令 JSON；timeout — 等待上限；policy — 成功判定；
    /// 输出: 成功为响应 `data`（`AcceptError` 时可为错误体），
    ///       失败为 [`ClientError`]（超时 / 错误响应 / 泵已停）。
    pub async fn ask(
        &self,
        value: Value,
        timeout: Duration,
        policy: ResponsePolicy,
    ) -> Result<Value, ClientError> {
        match self
            .actor
            .ask(CommandRequest {
                value,
                timeout,
                policy,
            })
            .await
        {
            Ok(data) => Ok(data),
            Err(SendError::HandlerError(e)) => Err(e),
            Err(SendError::Timeout(_)) => Err(ClientError::Timeout),
            Err(e) => Err(ClientError::Transport(e.to_string())),
        }
    }

    /// 登记一个额外的事件订阅方。
    ///
    /// 功能: 新建通道并经 actor 登记，自登记时点起接收事件扇出；
    /// 输入: 无；
    /// 输出: 新订阅方的事件接收端（`ClientEvent` 流，与主订阅同形）。
    pub async fn subscribe(&self) -> Receiver<ClientEvent> {
        let (tx, rx) = mpsc::channel();
        let _ = self.actor.tell(Subscribe { tx }).await;
        rx
    }

    /// 命令发送端克隆（组件 `handle_key` 的提交/中止通道；廉价克隆，
    /// 不借用整个客户端）。
    ///
    /// 功能: 暴露可跨线程的命令句柄；
    /// 输入: 无；
    /// 输出: [`CommandSender`] 克隆（会话存活期内有效，shutdown 后发送报错）。
    pub fn command_sender(&self) -> CommandSender {
        self.client.command_sender()
    }

    /// 关闭会话：杀 pi 子进程并停泵（幂等；此后 `ask` 报 Transport 错）。
    ///
    /// 功能: 向泵线程发 Shutdown（泵退出 → 事件通道断开 → 订阅方收到
    ///       补发的 `Closed`）；
    /// 输入: 无；
    /// 输出: 无返回值；副作用为子进程回收（可重复调用）。
    pub fn shutdown(&self) {
        self.client.shutdown();
    }
}

impl Drop for RpcSession {
    /// 会话句柄销毁即回收 pi 子进程（与显式 [`RpcSession::shutdown`] 幂等叠加）。
    fn drop(&mut self) {
        self.client.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write as _;
    use std::time::Instant;

    /// 写一个可执行 fake pi 脚本到独立临时目录（模式同 `rpc/client.rs` 测试）。
    ///
    /// 功能: 落盘 shell 脚本并赋可执行位；
    /// 输入: name — 目录区分名；body — 脚本正文（含 shebang）；
    /// 输出: 脚本绝对路径字符串（unix 上 chmod 755）。
    fn write_script(name: &str, body: &str) -> String {
        let dir = std::env::temp_dir().join(format!("gpui-actor-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fake-pi.sh");
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path.to_string_lossy().into_owned()
    }

    /// 正常应答脚本：get_state 回 data；`first` 先发无 id 事件再回 data。
    const OK_SCRIPT: &str = r#"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  cmd=$(printf '%s' "$line" | sed -n 's/.*"type":"\([^"]*\)".*/\1/p')
  if [ "$cmd" = "get_state" ]; then
    printf '{"type":"response","id":"%s","success":true,"data":{"model":"m1"}}\n' "$id"
  fi
  if [ "$cmd" = "first" ]; then
    printf '{"type":"agent_start"}\n'
    printf '{"type":"response","id":"%s","success":true,"data":{"a":1}}\n' "$id"
  fi
done
"#;

    /// ask 路由 + 事件扇出：ask 收到 data，主订阅与额外订阅都收到无 id 事件。
    ///
    /// 功能: 验证 spec「命令经路由层往返」「事件扇出」两条 scenario；
    /// 输入: 无（自建 fake pi）；
    /// 输出: 断言通过（probe 原样返回、ask==data、两订阅均见 agent_start）。
    #[tokio::test]
    async fn ask_routes_and_fanout_reaches_all_subscribers() {
        let path = write_script("route", OK_SCRIPT);
        let session = RpcSession::spawn(&path, &[], None).expect("spawn 应成功");
        assert_eq!(
            session.probe,
            serde_json::json!({"model": "m1"}),
            "probe 应原样返回 get_state 的 data"
        );

        let sub2 = session.subscribe().await;
        let data = session
            .ask(
                serde_json::json!({"type": "first"}),
                Duration::from_secs(5),
                ResponsePolicy::RequireSuccess,
            )
            .await
            .expect("ask 应收到响应");
        assert_eq!(data, serde_json::json!({"a": 1}), "ask 应返回 data");

        // 主订阅（spawn 时建立）收到无 id 事件。
        match session.events().recv_timeout(Duration::from_secs(2)) {
            Ok(ClientEvent::Event(v)) => assert_eq!(
                v.get("type").and_then(|t| t.as_str()),
                Some("agent_start"),
                "主订阅应收到 agent_start"
            ),
            other => panic!("主订阅应收到 agent_start，得到 {other:?}"),
        }
        // 额外订阅（subscribe 登记）同样收到。
        match sub2.recv_timeout(Duration::from_secs(2)) {
            Ok(ClientEvent::Event(v)) => assert_eq!(
                v.get("type").and_then(|t| t.as_str()),
                Some("agent_start"),
                "额外订阅应收到 agent_start"
            ),
            other => panic!("额外订阅应收到 agent_start，得到 {other:?}"),
        }

        session.shutdown();
    }

    /// 探活失败路径：返回可读 Err 且已杀子进程（spec: 启动探活与错误提示）。
    ///
    /// 功能: 验证 get_state 超时 → spawn 返回「探活失败」Err + 子进程被回收；
    /// 输入: 无（fake pi 记录自身 pid 且永不响应）；
    /// 输出: 断言通过（Err 文案、pid 文件出现、进程最终死亡）。
    #[tokio::test]
    async fn probe_failure_returns_err_and_kills_child() {
        let dir = std::env::temp_dir().join(format!(
            "gpui-actor-probefail-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let pidfile = dir.join("pid.txt");
        let body = format!(
            "#!/bin/sh\necho $$ > '{}'\nwhile IFS= read -r line; do sleep 1; done\n",
            pidfile.display()
        );
        let path = write_script("probefail", &body);

        let result = RpcSession::spawn_with_probe_timeout(
            &path,
            &[],
            None,
            Duration::from_millis(500),
        );
        let err = match result {
            Ok(_) => panic!("探活超时应返回 Err"),
            Err(e) => e,
        };
        assert!(err.contains("探活失败"), "Err 应含「探活失败」: {err}");

        // 子进程已被回收：pid 文件出现（脚本启动即写），随后进程消失。
        let deadline = Instant::now() + Duration::from_secs(2);
        let pid = loop {
            if let Ok(s) = fs::read_to_string(&pidfile) {
                break s.trim().to_string();
            }
            assert!(Instant::now() < deadline, "fake pi 未写出 pid 文件");
            thread::sleep(Duration::from_millis(20));
        };
        let dead_line = Instant::now() + Duration::from_secs(3);
        while process_alive(&pid) {
            assert!(
                Instant::now() < dead_line,
                "探活失败后子进程应被回收 (pid={pid})"
            );
            thread::sleep(Duration::from_millis(50));
        }

        let _ = fs::remove_dir_all(&dir);
    }

    /// 缺 bin 路径（task 6.1，参照 embed `spawn_failure` 测试）：不存在的
    /// 二进制 → 中文可读 Err（含「无法启动 pi」），不产生子进程。
    ///
    /// 输入: 构造的不存在路径；输出: 断言通过（Err 可读）。
    #[test]
    fn missing_binary_returns_readable_err() {
        let err = match RpcSession::spawn("/nonexistent/pi-binary-xyz", &[], None) {
            Ok(_) => panic!("缺 bin 应返回 Err"),
            Err(e) => e,
        };
        assert!(err.contains("无法启动 pi"), "Err 应中文可读: {err}");
    }

    /// shutdown 幂等 + 此后 ask 报错 + 订阅方收到 Closed（spec: 关闭感知）。
    ///
    /// 功能: 验证显式 shutdown 可重复调用、后续 ask 返回 Transport、
    ///       主订阅收到补发的 `Closed`；
    /// 输入: 无（fake pi 正常应答）；
    /// 输出: 断言通过。
    #[tokio::test]
    async fn shutdown_is_idempotent_and_fails_subsequent_asks() {
        let path = write_script("shutdown", OK_SCRIPT);
        let session = RpcSession::spawn(&path, &[], None).expect("spawn 应成功");

        session.shutdown();
        session.shutdown(); // 幂等，不 panic

        let err = session
            .ask(
                serde_json::json!({"type": "first"}),
                Duration::from_secs(1),
                ResponsePolicy::RequireSuccess,
            )
            .await
            .expect_err("shutdown 后 ask 应报错");
        assert!(
            matches!(err, ClientError::Transport(_)),
            "shutdown 后应为 Transport 错，得到 {err:?}"
        );

        match session.events().recv_timeout(Duration::from_secs(2)) {
            Ok(ClientEvent::Closed) => {}
            other => panic!("订阅方应收到补发的 Closed，得到 {other:?}"),
        }
    }

    /// 查询本 pid 是否存活（`ps -p <pid> -o pid=`；unix 测试口径）。
    ///
    /// 功能: 进程存活探测；
    /// 输入: pid — 目标进程号（十进制字符串）；
    /// 输出: true = 存活（ps 成功且有输出），false = 已消失/探测失败。
    fn process_alive(pid: &str) -> bool {
        std::process::Command::new("ps")
            .args(["-p", pid, "-o", "pid="])
            .output()
            .map(|o| o.status.success() && !o.stdout.is_empty())
            .unwrap_or(false)
    }
}
