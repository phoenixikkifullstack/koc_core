# KOC Core

Rust 实现的 KOC 游戏自动化核心库，包含 token 管理、WebSocket 通信、每日任务调度、微信扫码登录等功能。

## 项目结构

```
koc_core/
├── src/
│   ├── lib.rs              # 库入口, KocCore 核心结构体
│   ├── bon.rs              # BON (Binary Object Notation) 编解码
│   ├── crypto.rs           # 加解密 (XOR / LZ4)
│   ├── protocol.rs         # 游戏消息协议 (ProtoMsg 解析/创建)
│   ├── http_client.rs      # HTTP 客户端 (authuser / serverlist)
│   ├── websocket.rs        # WebSocket 客户端 (心跳, 请求/响应匹配)
│   ├── proxy.rs            # 实时 WebSocket payload 保真 relay
│   ├── proxy_capture.rs    # Capture schema、实时解析、关联和命令目录
│   ├── kpi.rs              # 高层Api (登录, 155个游戏命令, 每日/周期任务)
│   ├── error_codes.rs      # 游戏错误码映射 + is_done_error 判定
│   ├── config.rs           # 配置文件解析 (YAML) + ConfigWatcher 热加载
│   ├── state.rs            # 运行时状态持久化 (JSON) + AppState/RoleState
│   ├── scheduler.rs        # 批量调度器 (并发控制, 轮次管理, 热加载)
│   ├── hortor_crypto.rs    # Hortor 平台 payload 加密 (cipher table + XOR)
│   ├── wx_login.rs         # 微信扫码登录 + Hortor 登录 + bin 生成
│   ├── logging.rs          # tracing 日志初始化 (console + 文件)
│   ├── koc_batch.rs        # [bin] 批量每日任务调度器 CLI
│   ├── token_gen.rs        # [bin] Token/Bin 文件生成工具 CLI
│   ├── koc_cli.rs          # [bin] 手动任务验证 CLI (verify/study/tower/evotower/monthly/car/skinc)
│   └── koc_proxy.rs        # [bin] 实时协议 relay 和 capture inspector
├── run_koc_tasks.ps1       # 综合任务交互执行脚本
├── run_koc_cli.ps1         # 全新双层菜单交互脚本
├── examples/
│   ├── parse_bin.rs        # 解析 bin 文件内容
│   ├── server_list.rs      # 获取角色列表 (带调试)
│   ├── server_list_json.rs # 导出完整 serverlist 为 JSON
│   └── full_flow.rs        # 单角色完整流程演示
├── config.yaml             # 调度器配置示例
├── Cargo.toml
├── README.md                # English documentation
└── README-zh.md             # 中文文档
```

## 编译

```bash
# 编译所有
cargo build --release --bins --examples

# 只编译调度器
cargo build --release --bin koc_batch

# 只编译 token 生成工具
cargo build --release --bin token_gen

# 只编译实时协议代理
cargo build --release --bin koc_proxy

# 只编译手动验证工具
cargo build --release --bin koc_cli
```

## 工具一: token_gen - Token/Bin 文件生成

独立的 token/bin 文件生成工具，当前支持微信扫码方式，后续可扩展更多方式。

### 微信扫码生成 bin

```bash
# 基本用法: 扫码后 bin 保存到 config.yaml 中配置的 bin_output_dir
./token_gen scan

# 指定输出目录
./token_gen scan -o /data/bins/

# 生成后打印可追加到 config.yaml 的 YAML 片段
./token_gen scan --add-to-config

# 指定配置文件
./token_gen -c /path/to/config.yaml scan
```

**流程:**

```
终端显示 ASCII 二维码 → 微信扫码确认 → 提取 OAuth code
→ Hortor 平台登录 (加密 payload) → 获取用户凭证
→ 构造 bin 数据 (BON 编码 + 加密) → 保存到文件
```

**输出示例:**

```
=== KOC Token Generator ===
[token_gen] Output dir: bins/

[token_gen] Fetching WeChat QR code...
[token_gen] Please scan with WeChat (120s timeout):

  █▀▀▀▀▀█ ▄█▀▄ █▀▀▀▀▀█
  █ ███ █ ▀▄▀▄ █ ███ █
  ...

[token_gen] Got OAuth code: 091y...IDG2 (len=32)
[token_gen] Scan confirmed! User: phoenix
[token_gen] Hortor login successful
[token_gen] Bin saved: bins/phoenix.bin (1061 bytes)
[token_gen] Done!
```

### CLI 参数

```
token_gen [OPTIONS] <COMMAND>

Commands:
  scan    微信扫码生成 bin 文件

Options:
  -c, --config <CONFIG>    配置文件路径 [default: config.yaml]
  -h, --help               显示帮助

scan 子命令:
  -o, --output <DIR>       输出目录 (覆盖 config 中的 bin_output_dir)
      --add-to-config      打印可追加到 config.yaml 的 YAML 片段
```

## 工具二: koc_cli - 手动任务验证 CLI

用于手动验证单个 bin / 单角色流程，功能稳定后可迁移到 batch 调度。

- `verify` 子命令用于快速验 bin 和列角色
- `study` 子命令复用核心模块（`KocCore` + `GameClient` + `study::run_study`）执行完整答题流程
- `tower` 子命令复用核心模块（`fight_starttower/tower_getinfo/tower_claimreward`）执行自动爬塔
- `evotower` 子命令复用核心模块（`evotower_getinfo/evotower_readyfight/evotower_fight`）执行自动爬怪异塔
  - 执行顺序：先领取免费道具（mergebox）→ 再爬怪异塔 → 最后执行一轮 merge 合成
- `skinc` 子命令用于手动执行换皮挑战，自动校验开放时间、当值 Boss 并过滤已通关层数
- `monthly` 子命令用于查询月度任务进度，并按需补齐普通鱼竿钓鱼/竞技场任务
- `car` 子命令用于手动发车/收车，执行智能发车逻辑（含刷新券判断与护卫分配）
- `info` 子命令用于查询 role info 或 evotower info 的完整 JSON
- `daily` 子命令用于手动执行每日任务，支持单角色/按组/全量三种模式
- `group` 子命令用于查看 config.yaml 中定义的角色分组

```bash
# 验证 bin 可用性并列出角色
./koc_cli verify --bin bins/phoenix.bin

# 验证并检查指定 serverId 是否存在（可选）
./koc_cli verify --bin bins/phoenix.bin --server-id 12489

# 执行答题任务（指定 serverId）
./koc_cli study --bin bins/phoenix.bin --server-id 13007

# 强制答题（忽略本周已完成状态）
./koc_cli study --bin bins/phoenix.bin --server-id 13007 --force

# 自动爬塔（直到没体力/通关/达到最大次数）
./koc_cli tower --bin bins/phoenix.bin --server-id 13007

# 自动爬塔（自定义参数）
./koc_cli tower --bin bins/phoenix.bin --server-id 13007 --max-climb 60 --interval-ms 1200 --refresh-every 5

# 自动爬怪异塔（默认连续请求失败3次才停止）
./koc_cli evotower --bin bins/phoenix.bin --server-id 13007

# 自动爬怪异塔（自定义参数）
./koc_cli evotower --bin bins/phoenix.bin --server-id 13007 --max-climb 120 --interval-ms 1000 --failure-limit 3

# 自动爬怪异塔并调整 merge 循环参数
./koc_cli evotower --bin bins/phoenix.bin --server-id 13007 --merge-max-loops 30 --merge-delay-ms 600

# 自动执行换皮挑战 (单角色)
./koc_cli skinc --bin bins/phoenix.bin --server-id 13007

# 自动执行换皮挑战 (按分组)
./koc_cli skinc --group 大号

# 查询月度任务进度（默认行为）
./koc_cli monthly --bin bins/phoenix.bin --server-id 13007

# 只查询月度钓鱼进度
./koc_cli monthly --bin bins/phoenix.bin --server-id 13007 --mode fish

# 按当前日期进度补齐竞技场月度任务
./koc_cli monthly --bin bins/phoenix.bin --server-id 13007 --topup --mode arena

# 直接补满鱼竿与竞技场月度任务
./koc_cli monthly --bin bins/phoenix.bin --server-id 13007 --topup --mode all --complete

# 智能发车 (周一/二/三 20:00前可用, 含刷新券/大奖判断)
./koc_cli car --bin bins/phoenix.bin --server-id 13007

# 只收车 (不发车)
./koc_cli car --bin bins/phoenix.bin --server-id 13007 --action claim

# 查看 role info (默认)
./koc_cli info --bin bins/phoenix.bin --server-id 13007

# 查看 evotower info
./koc_cli info --bin bins/phoenix.bin --server-id 13007 --type evotower

# 执行每日任务 (单角色)
./koc_cli daily --bin bins/phoenix.bin --server-id 13007

# 执行每日任务 (按分组，不执行发车/扭蛋)
./koc_cli daily --group 大号 --skip-car --skip-gacha

# 执行每日任务 (全量)
./koc_cli daily --force-all

# 查看角色分组
./koc_cli group
```

### CLI 参数

```text
koc_cli [OPTIONS] <COMMAND>

Commands:
  verify   验证 bin 并列出角色
  study    执行答题周任务
  monthly  查询或补齐月度任务
  tower    自动爬塔任务
  evotower 自动爬怪异塔任务
  skinc    自动换皮挑战任务
  car      智能发车/收车
  info     查询 role info 或 evotower info
  daily    执行每日任务 (单角色/分组/全量)
  gacha    免费扭蛋
  group    查看角色分组

Options:
  -c, --config <CONFIG>        YAML 配置文件路径 [default: config.yaml]

verify 子命令:
  --bin <BIN>                 bin 文件路径
  --server-id <SERVER_ID>     可选，检查目标角色是否存在

study 子命令:
  --bin <BIN>                 bin 文件路径
  --server-id <SERVER_ID>     目标角色 serverId（必填）
  --force                     强制执行答题，忽略本周已完成状态

monthly 子命令:
  --bin <BIN>                 bin 文件路径
  --server-id <SERVER_ID>     目标角色 serverId（必填）
  --mode <MODE>               fish|arena|all（默认 all）
  --topup                     显式执行补齐；默认仅查询
  --complete                  与 --topup 一起使用，直接补满到月度上限
  --arena-safety-max <N>      竞技场补齐安全上限（默认 100）
  --fish-batch-size <N>       付费钓鱼每批次数（默认 10）
  --no-claim-fish-point       关闭鱼竿累计奖励自动领取

tower 子命令:
  --bin <BIN>                 bin 文件路径
  --server-id <SERVER_ID>     目标角色 serverId（必填）
  --max-climb <N>             最大爬塔次数（默认 100）
  --interval-ms <MS>          每次爬塔间隔毫秒（默认 1000）
  --refresh-every <N>         每 N 次强制刷新 role 信息（默认 5）
  --no-auto-claim             关闭自动领取上座塔奖励

evotower 子命令:
  --bin <BIN>                 bin 文件路径
  --server-id <SERVER_ID>     目标角色 serverId（必填）
  --max-climb <N>             最大爬塔次数（默认 100）
  --interval-ms <MS>          每次爬塔间隔毫秒（默认 1000）
  --refresh-every <N>         每 N 次刷新 evotower 信息（默认 3）
  --failure-limit <N>         连续请求失败停止阈值（默认 3）
  --no-auto-claim-task        关闭自动领取怪异塔每日任务奖励
  --no-auto-claim-reward      关闭自动领取怪异塔章节奖励
  --merge-max-loops <N>       merge 合成最大循环次数（默认 20）
   --merge-delay-ms <MS>       merge 流程循环间隔毫秒（默认 500）

skinc 子命令:
  --bin <BIN>                 单角色模式: bin 文件路径
  --server-id <SERVER_ID>     单角色模式: 目标 serverId
  --group <NAME>              分组模式: 执行 config.yaml 中该组所有角色
  --force-all                 全量模式: 执行 config.yaml 中所有角色

car 子命令:
  --bin <BIN>                 bin 文件路径
  --server-id <SERVER_ID>     目标角色 serverId（必填）
  --action <ACTION>           操作类型: send(发车+收车) 或 claim(仅收车) [default: send]

info 子命令:
  --bin <BIN>                 bin 文件路径
  --server-id <SERVER_ID>     目标角色 serverId（必填）
  --type <TYPE>               信息类型: role (默认) 或 evotower

daily 子命令:
  --bin <BIN>                 单角色模式: bin 文件路径
  --server-id <SERVER_ID>     单角色模式: 目标 serverId
  --group <NAME>              分组模式: 执行 config.yaml 中 groups 定义的该组所有角色
  --force-all                 全量模式: 执行 config.yaml 中所有角色
  --skip-car                  跳过发车/收车
  --skip-gacha                跳过免费扭蛋

gacha 子命令:
  --bin <BIN>                 bin 文件路径
  --server-id <SERVER_ID>     目标角色 serverId（必填）

group 子命令:
  (无参数, 列出 groups 定义及其包含的角色)
```

evotower 领取说明:
- `evotower_claimtask` 每次运行会先从服务端 `taskClaimMap` 恢复当天已领任务状态
- `evotower_claimtask` 按爬塔次数递增触发：第 3 次尝试 `task_id=1`，第 6 次尝试 `task_id=2`，第 10 次尝试 `task_id=3` (3/3/4 间隔)
- `task_id` 成功领取后，本次运行内不再重复领取
- 错误码 `12200050` 视为已领取，不再重复请求
- 错误码 `12200040` 视为条件不足，后续仍可在满足条件时再尝试
- `evotower_readyfight` 若返回 `12200020`，会尝试先领取当前章节奖励后再重试

skinc 说明:
- 自动验证账号是否处于换皮挑战的 7 天有效期内（解析 `actId` 校验）
- 基于系统实际星期动态选择当天开放的 Boss（周五Boss1 ~ 周三Boss6，周四Boss1~6全开放）
- 根据服务端返回的 `levelRewardMap` 自动排除已打通关（第8层）的 Boss
- 战斗过程若失败自动重试；单一 Boss 连续失败 3 次则跳过以防死循环

monthly 说明:
- 默认行为是查询，不执行任何补齐
- 查询输出会包含当前普通鱼竿余量与竞技场门票余量
- `--topup` 才会执行补齐动作
- 默认补齐到“当前日期应有进度”；`--complete` 会直接补满到月度上限
- 若当前进度已达标或超额，不会继续补齐
- `fish` 不依赖 YAML 阵容配置
- `arena`/`all` 会读取 YAML 中的竞技场阵容，并在任务后恢复原阵容

car 说明:
- 仅在周一/二/三 20:00 前可用（发车窗口）
- `send` 模式先收车再发车，`claim` 模式仅收车
- 发车策略: 周一/二含刷新券即发、待刷新券时用刷新券刷新；周三刷大奖
- 红色及以上品质车辆自动分配军团护卫（护卫上限每人 4 次）
- 收车后自动升级发动机并领取零件消耗奖励

info 说明:
- 登录后查询指定 role 的完整服务器数据，输出格式化 JSON 到 stdout
- `--type role` 输出 role_getroleinfo 结果（含挂机/盐罐/塔/统计等）
- `--type evotower` 输出 evotower_getinfo 结果（含体力/塔层/任务状态等）

daily 说明:
- 三种模式: 单角色 (`--bin`/`--server-id`)、按组 (`--group`)、全量 (`--force-all`)
- 复用 scheduler 的核心逻辑，读取 config.yaml 的 formations/dream_shop 配置
- server 侧已完成的任务会自动跳过
- 不读写 state.json，每次执行都是独立会话

gacha 说明:
- 调用 `gacha_drawreward` 执行一次免费扭蛋
- 基于 server 侧 `statisticsTime["gacha:free"]` 判断今日是否已扭过
- 已扭过则 skip 不再调 API

group 说明:
- 列出 config.yaml 中 `groups` 声明的所有分组及其包含的角色
- 无分组时列出全部角色
- 适合重定向保存: `./koc_cli info ... > role.json`

## 辅助交互脚本 (Scripts)

在 Windows 环境下，如果你不想每次手动敲击繁杂的命令行参数，可以直接运行项目根目录下的 PowerShell 脚本进行交互式批量操作：

- `run_koc_tasks.ps1`: 综合交互入口。运行后可在终端菜单中勾选执行 `tower`、`evotower`、`car` 或一键执行全部，并配合预设好的 ID 分组批量运行。
- `run_koc_cli.ps1`: 全新的双层状态机菜单脚本。第一层选择目标子命令，第二层通过解析 `config.yaml` 动态生成结构化的 `Bin + ServerId` 列表，支持精确到单号或整组执行，执行后保留日志并支持无缝返回。

## 工具三: koc_batch - 批量每日任务调度器

常驻运行的后台调度器，管理多个 bin 文件中所有角色的每日任务和周期任务。

### 启动

```bash
# 前台运行
./koc_batch -c config.yaml -s state.json

# 后台运行
nohup ./koc_batch -c config.yaml -s state.json > koc.log 2>&1 &

# Ctrl+C 优雅退出 (等待当前任务完成)
```

### CLI 参数

```
koc_batch [OPTIONS]

Options:
  -c, --config <CONFIG>    配置文件路径 [default: config.yaml]
  -s, --state <STATE>      状态文件路径 [default: state.json]
  -h, --help               显示帮助
```

### 调度策略

程序启动后进入主循环，包含三类任务:

**A. 每日任务** (到达 `schedule_time` 时触发，每天一次)

| 任务 | 说明 |
|------|------|
| 分享游戏 | 挂机加钟 |
| 赠送好友金币 | 批量赠送 |
| 免费招募 | 每日1次 |
| 智能挂机 | 剩余<=1h则领取+加钟, 上限<=8h则加钟, 否则攒时间 |
| 免费点金 | 每日3次 |
| 开启宝箱 | 木质宝箱x10 |
| 黑市采购 | 每日1次 |
| 盐罐 | 智能处理: 已停止则领取, stop & start 续时间 |
| 竞技场 | 进入 → 获取对手 → 战斗x3 (关卡≥400) |
| Boss战 | 每日3次 (按星期选BOSS) |
| 俱乐部Boss | 每日免费2次 (智能判断剩余次数) |
| 福利签到 | 每日1次 |
| 俱乐部签到 | 每日1次 |
| 折扣礼包 | 每日1次 |
| 卡牌礼包 | 免费卡 + 永久卡 |
| 珍宝阁 | 每日免费奖励 |
| 免费钓鱼 | 每日3次 |
| 灯神扫荡 | 魏蜀吴群各1次 + 免费扫荡卷x3 (关卡≥3000) |
| 免费扭蛋 | 每日1次 |
| 功法残卷 | 领取挂机收益 (关卡>8000) |
| 任务积分 | 领取每日/每周任务奖励 |
| 邮件 | 一键领取所有附件 |
| 俱乐部发车 | 周一/二/三 智能发车, 含刷新券/大奖判断 |
| 俱乐部收车 | 已发车4小时后自动收车 + 升级发动机 |

**B. 周期任务** (按时间阈值触发，每轮检查)

| 任务 | 触发条件 | 默认阈值 |
|------|----------|----------|
| 挂机加钟+领取 | 剩余<=1h或到期则领取+加钟4次; 上限<=8h则只加钟 | 动态判断 |
| 盐罐续时间 | 剩余时间 <= 阈值 | 1 小时 |
| 功法残卷领取 | 距上次领取 >= 间隔 | 4 小时 |
| 俱乐部收车 | 每日首次收车后 >=4h 再次收车 | 动态判断 |
| 咸将塔 | 能量回满10后爬塔, fight_starttower 循环 | 动态判断 |
| 怪异塔 | 黑市周能量回满10后爬塔, 含daily task/chapter reward/mergebox (关卡≥7000) | 动态判断 |

**C. 每周任务** (在 `weekly_schedule_day` 当天执行一次)

| 任务 | 说明 |
|------|------|
| 答题 | 自动答题 (10题), 完成即标记本周 done |

### 智能特性

- **状态持久化**: 每个角色的每日任务子项独立标记完成状态，保存到 `state.json`，重启后不重复执行
- **智能跳过**: 服务端已完成的任务自动跳过；"已完成"类错误码标记 done 不再重试
- **配置热加载**: 每轮检查 `config.yaml` 和 bin 文件的修改时间，变化自动重载
- **State 联动**: 配置增删 bin/role 时，state.json 自动同步 (新增空状态, 移除孤儿)
- **并发控制**: Semaphore 控制最大并发 WebSocket 连接数
- **结构化日志**: 基于 `tracing`，并发场景自动携带 `round/role` 上下文
- **双输出**: 同时输出到 console 和 `logs/` 按天滚动文件

### 日志输出示例

```
2026-04-16 13:13:51  INFO RT{R=5 role=erqiao/15001-0}: task: [OK] Boss战 #3/3 (id=9903)
2026-04-16 13:13:51  INFO RT{R=5 role=erqiao/12980-0}: task: [~~] 免费礼包卡 code=-10006 error=[-10006] Today's reward already claimed or attempts exhausted
2026-04-16 13:13:52  WARN RT{R=5 role=erqiao/15001-0}: task: [X] 领取功法残卷 code=800040 error=[800040] unknown code
```

日志标记含义:
- `[OK]` 执行成功
- `[~~]` 已完成, 跳过
- `[X]` 执行失败 (附错误码和英文描述)

日志时间与格式:
- 时间为 `UTC+8`，格式 `YYYY-MM-DD HH:MM:SS`
- console 默认无颜色（便于重定向与 grep）

日志级别与输出:
- 默认级别 `info`，可通过环境变量调节：`RUST_LOG=debug ./koc_batch`
- 文件日志目录：`logs/`（按天滚动）

## 工具四: koc_proxy - 实时协议解析

`koc_proxy` 会原样转发 WebSocket 消息，同时将每个 Binary message 的副本交给现有 XOR/LZ4/BON 协议栈实时解析。解析失败不会修改或阻塞游戏流量。Relay 仅允许监听 loopback 地址，也不会记录 WebSocket 握手中的认证 query。

启动实时 relay，并可选保存原始记录和命令目录：

```bash
cargo run --bin koc_proxy -- relay \
  --listen 127.0.0.1:8787 \
  --decode \
  --record captures/session.jsonl \
  --catalog captures/catalog.json
```

在另一个终端将本项目的 WebSocket 客户端指向 relay。Token 仍由正常登录流程传递，不会出现在命令行参数中：

```bash
KOC_WS_BASE_URL=ws://127.0.0.1:8787 \
  cargo run --bin koc_cli -- info --bin bins/<account>.bin --server-id <ID>
```

如果没有设置 `KOC_WS_BASE_URL`，`koc_cli` 会直接连接上游服务器，relay 不会收到任何流量。Session JSONL 和 catalog 快照会在 relay 运行期间实时 flush，无需等到退出后再查看。

需要长期对当前 shell 生效时，必须 export，并确认子进程可以看到该变量。仅执行 `echo $KOC_WS_BASE_URL` 不能证明变量已经导出：

```bash
export KOC_WS_BASE_URL=ws://127.0.0.1:8787
env | grep '^KOC_WS_BASE_URL='
```

实时解析默认开启，支持 JSON 输出以及命令、方向过滤：

```bash
cargo run --bin koc_proxy -- relay --format json --cmd 'activity_*'
cargo run --bin koc_proxy -- relay --direction server-to-client
```

协议解析器改进后，可以重新解析历史记录或生成命令结构目录：

```bash
cargo run --bin koc_proxy -- decode --input captures/session.jsonl
cargo run --bin koc_proxy -- catalog \
  --input captures/session.jsonl \
  --output captures/catalog.json
```

`--record` 保存的是 raw payload JSONL，因此文件中不会直接出现明文 `cmd`。`--decode` 表示把实时解析结果输出到 relay 终端。需要查看已保存 session 的命令时间线时使用 `koc_proxy decode`；只查看命令名可以查询 catalog JSON：

```bash
./target/release/koc_proxy decode \
  --input capture/session.jsonl \
  --format json \
  --cmd '*tower*'

jq -r '.commands | keys[]' capture/catalog.jsonl
```

外部 TLS MITM 可以把已经解除 TLS、WebSocket framing 和 masking 的完整消息转换为 capture-record JSONL，再通过 stdin 实时输入。`koc_proxy` 本身不负责对任意 TLS 连接签发证书：

```bash
external-adapter | cargo run --bin koc_proxy -- inspect --stream
```

Unix 下 capture 文件权限为 `0600`，且 `captures/` 已被 Git 忽略。默认输出会脱敏常见 token/session 字段，只有显式传入 `--show-sensitive` 才显示原值。原始 payload 仍可能包含角色隐私数据，不应提交到仓库。

Relay 启动时会覆盖已有的普通 capture/catalog 文件，但会拒绝 symlink 和非文件路径。远程 upstream 必须使用 `wss://`，明文 `ws://` 只允许指向 loopback 开发服务器。

## 配置文件 config.yaml

```yaml
# 最大并发 WebSocket 连接数
concurrency: 5

# 每个 role 启动之间的间隔 (ms, 防止服务端限流)
delay_between_ms: 2000

# 每日任务执行时间 (24小时格式 HH:MM)
schedule_time: "06:00"

# 周五每日任务开始时间覆盖 (24小时格式 HH:MM)
friday_daily_start_time: "12:10"

# 每日任务最大重试次数
max_daily_retries: 1

# 俱乐部发车功能 (周一/二/三 schedule_time~20:00)
car_enabled: true

# 每周任务执行日 (Mon/Tue/Wed/Thu/Fri/Sat/Sun)
weekly_schedule_day: "Sat"

# 答题任务 (默认 true)
study_enabled: true

# 咸将塔 (默认 true, 24/7 能量回满10后爬)
tower_enabled: true

# 怪异塔 (默认 true, 仅黑市周能量回满10后爬)
evotower_enabled: true

# 免费扭蛋 (默认 true)
gacha_enabled: true

# 主循环检查间隔 (秒)
check_interval_secs: 1800

# 挂机: 已挂机超过多少小时触发领取+加钟
hangup_threshold_hours: 8.0

# 盐罐: 剩余时间低于多少小时触发 stop & start 续时间
bottle_threshold_hours: 1.0

# 功法残卷: 每隔多少小时领取一次
legacy_interval_hours: 4.0

# bin 文件输出目录 (token_gen 扫码生成的 bin 存放位置)
bin_output_dir: bins/

# batch/profile 使用的默认 bin 目录
default_bin_path: bins

formations:
  defaults:
    arena: 1
    tower: 1
    evotower: 1
    boss_daily: 1
    boss_legion: 1

# 可选: 对单个 role 覆盖特定场景阵容
# bins:
#   - bin: account.bin
#     roles:
#       - server_id: 10000
#         formations:
#           tower: 2
#           evotower: 3
#           boss_daily: 4
#           boss_legion: 4
#         dream_shop:
#           enabled: true
#           purchase_list:
#             - "1-5" # 初级商人-咸神门票
#             - "1-6" # 初级商人-咸神火把
#             - "3-2" # 高级商人-黄金鱼竿

dream_shop_presets:
  basic_daily_shop:
    enabled: true
    purchase_list:
      - "1-5" # 初级商人-咸神门票
      - "1-6" # 初级商人-咸神火把
      - "3-1" # 高级商人-铂金宝箱
      - "3-2" # 高级商人-黄金鱼竿

# 维护窗口配置: 命中后整轮跳过 (不尝试连接服务器)
maintenance_windows:
  - weekday: Fri
    start_time: "05:00"
    end_time: "07:00"
  - weekday: Sat
    start_time: "19:15"
    end_time: "21:15"
  - weekday: Sun
    start_time: "19:15"
    end_time: "20:45"

# === 角色分组 (CLI --group 使用) ===
groups: []
#   - 大号
#   - 小号

# === bin 文件列表 ===
bins:
  - bin: account.bin
    roles:
      - server_id: 10000  # role 必须显式列出
        # group: 大号         # 可选, groups 列表中声明的分组名
        formations:
          tower: 2
          evotower: 2
      - server_id: 1010000
      - server_id: 2010000
```

### bins 配置说明

- `bin`: bin 文件名（batch/profile 使用）
- `roles[].server_id`: 要处理的角色列表，必须显式列出
- `roles[].formations`: 可选，仅覆盖该 role 的特定场景阵容
- `roles[].dream_shop`: 可选，梦境商店购买配置（默认关闭），支持 inline 配置或引用 `dream_shop_presets` 名称
- `roles[].group`: 可选，角色分组标签（必须在 `groups` 列表中声明，否则 warn 跳过）
- `verify/study` 不读取 profile，只使用显式 `--bin` 文件路径
- `tower/evotower` 读取 YAML 中的阵容配置，并在任务前切换阵容、任务后恢复原阵容

### 阵容场景键

- `arena`: 竞技场
- `tower`: 咸将塔
- `evotower`: 怪异塔
- `boss_daily`: 每日 Boss
- `boss_legion`: 军团 Boss

说明:
- 未配置 `roles[].formations` 时，使用 `formations.defaults`
- 目前 `koc_batch` 已接入 `arena` / `boss_daily` / `boss_legion`
- `koc_batch` daily 已包含咸王梦境（周日/周一/周三/周四）
- `koc_batch` daily 可按 role 配置执行梦境商店购买（与咸王梦境相同开放日）
- `koc_cli tower` / `koc_cli evotower` 已接入对应场景阵容
- `verify/study` 不切阵容

### 梦境商店 `purchase_list` 映射

格式：`merchantId-itemIndex`

初级商人（1）:
- `1-0`: 进阶石
- `1-1`: 精铁
- `1-2`: 木质宝箱
- `1-3`: 青铜宝箱
- `1-4`: 普通鱼竿
- `1-5`: 咸神门票
- `1-6`: 咸神火把

中级商人（2）:
- `2-0`: 梦魇晶石
- `2-1`: 进阶石
- `2-2`: 精铁
- `2-3`: 黄金宝箱
- `2-4`: 黄金鱼竿
- `2-5`: 招募令
- `2-6`: 橙将碎片
- `2-7`: 紫将碎片

高级商人（3）:
- `3-0`: 梦魇晶石
- `3-1`: 铂金宝箱
- `3-2`: 黄金鱼竿
- `3-3`: 招募令
- `3-4`: 红将碎片
- `3-5`: 橙将碎片
- `3-6`: 红将碎片
- `3-7`: 普通鱼竿

说明:
- `purchase_list` 配置的是商品类型，不是商店位置 `pos`
- 运行时会根据 `role.dungeon.merchant` 动态解析出当前位置，并按 `merchantId` 升序、`pos` 降序购买
- 仅在周日/周一/周三/周四开放日执行，且需要 `levelId >= 1000`

### 时间窗口说明

- `schedule_time`: 常规每日任务开始时间
- `friday_daily_start_time`: 周五覆盖开始时间（默认 `12:10`）
- `maintenance_windows`: 维护窗口配置，窗口内整轮跳过，不尝试连接服务器
- 若窗口有重复/重叠/相邻，启动时会自动去重并合并；无效窗口（时间格式错误、`start>=end`）会被丢弃

### serverId 编码规则

```
serverId = 实际内部ID + 角色序号 * 1000000
实际区服号 = 内部ID - 27

示例:
  13007         → 12980服, 0号角色
  1013007       → 12980服, 1号角色
  2013007       → 12980服, 2号角色
```

## Examples

### parse_bin - 解析 bin 文件

```bash
# 需要在 koc_core/ 目录下, bin 文件路径在代码中指定
cargo run --example parse_bin
```

解密并显示 bin 文件的字段内容 (platform, info, serverId 等)。

### server_list - 获取角色列表

```bash
cargo run --example server_list
```

向服务器发送 bin 数据, 获取并显示该账号下所有角色的信息 (名称, 区服, 战力, 等级)。

### server_list_json - 导出完整 JSON

```bash
cargo run --example server_list_json
```

将服务器返回的完整 serverlist 数据导出为 `server_list.json` 文件, 包含所有区服信息和角色详情。

### full_flow - 单角色完整流程

```bash
cargo run --example full_flow
```

演示单个角色的完整自动化流程:
1. 获取角色列表
2. 选择最强角色
3. 获取 token
4. WebSocket 登录 (含 randomSeed 同步)
5. 执行每日任务 (智能跳过已完成)
6. 断开连接

## 游戏命令 API

`GameClient` 提供 155 个游戏命令方法, 按功能分组:

| 分组 | 命令数 | 示例 |
|------|--------|------|
| 系统/登录 | 7 | `system_signinreward`, `system_buygold` |
| 好友 | 1 | `friend_batch` |
| 武将 | 8 | `hero_recruit`, `hero_heroupgradelevel` |
| 物品/宝箱 | 3 | `item_openbox`, `item_batchclaimboxpointreward` |
| 竞技场 | 3 | `arena_startarea`, `arena_getareatarget` |
| 战斗 | 7 | `fight_startboss`, `fight_startareaarena` |
| 任务 | 3 | `task_claimdailypoint`, `task_claimdailyreward` |
| 商店 | 4 | `store_purchase`, `store_refresh` |
| 俱乐部/军团 | 20+ | `legion_signin`, `fight_startlegionboss` |
| 邮件 | 4 | `mail_claimallattachment`, `mail_getlist` |
| 答题 | 3 | `study_startgame`, `study_answer` |
| 神器/钓鱼 | 4 | `artifact_lottery`, `artifact_load` |
| 灯神 | 2 | `genie_sweep`, `genie_buysweep` |
| 盐罐 | 3 | `bottlehelper_claim`, `bottlehelper_start` |
| 咸将塔 | 2 | `tower_getinfo`, `fight_starttower` |
| 怪异塔 | 6 | `evotower_getinfo`, `evotower_fight` |
| 咸王宝库 | 4 | `bosstower_getinfo`, `bosstower_startboss` |
| 合成箱 | 7 | `mergebox_getinfo`, `mergebox_automergeitem` |
| 车辆 | 7 | `car_getrolecar`, `car_send` |
| 功法 | 6 | `legacy_claimhangup`, `legacy_gift_send` |
| 装备 | 3 | `equipment_quench`, `equipment_confirm` |
| 其他 | 40+ | `presetteam_saveteam`, `rank_getserverrank` ... |

所有命令也可以通过通用接口调用:

```rust
// 等待响应
let result = game.cmd("any_command", json!({"key": "value"})).await?;

// 不等响应
game.cmd_fire("any_command", json!({})).await?;
```

## 错误码

内置 30+ 条游戏错误码英文说明, 分为两类:

**已完成/不可操作** (标记 done, 不重试):
- `400190` No check-in rewards available to claim
- `2300190` Already checked in today
- `200160` Feature not unlocked
- `-10006` Today's reward already claimed or attempts exhausted
- ... (共 25 条)

**其他错误** (不标记 done, 下轮重试):
- `200400` Action performed too quickly; try again later
- 网络超时等

## 技术架构

```
┌──────────────────────────────────────────────┐
│              token_gen CLI                    │
│  (微信扫码 / 其他方式生成 bin)                 │
└──────────────────┬───────────────────────────┘
                   │ 生成 .bin 文件
                   ▼
┌──────────────────────────────────────────────┐
│            koc_batch CLI                     │
│  (常驻调度器, 读取 config.yaml)               │
│                                              │
│  ┌─ Scheduler ──────────────────────────┐    │
│  │  主循环 (每60s检查)                    │    │
│  │  ├─ 配置热加载 (ConfigWatcher)        │    │
│  │  ├─ State 联动 (sync_with_roles)     │    │
│  │  └─ 并发执行 (Semaphore)             │    │
│  └──────────────────────────────────────┘    │
│           │                                  │
│     ┌─────┼─────┐                            │
│     ▼     ▼     ▼                            │
│   tokio  tokio  tokio  (并发 role 任务)       │
│     │     │     │                            │
│     ▼     ▼     ▼                            │
│  ┌──────────────────┐                        │
│  │   GameClient      │                        │
│  │  ├─ login()       │  WebSocket 连接         │
│  │  ├─ daily_tasks() │  每日任务 (带状态标记)   │
│  │  ├─ periodic()    │  周期任务 (挂机/盐罐)   │
│  │  └─ disconnect()  │                        │
│  └──────────────────┘                        │
│           │                                  │
│     ┌─────┼─────┐                            │
│     ▼     ▼     ▼                            │
│   BON   Crypto  WebSocket                    │
│  编解码  加解密  客户端                        │
└──────────────────────────────────────────────┘
           │
           ▼
    游戏服务器 (WSS)
```

## 依赖

| Crate | 用途 |
|-------|------|
| `tokio` | 异步运行时 (WebSocket, HTTP, 定时器) |
| `tokio-tungstenite` | WebSocket 客户端 (rustls TLS) |
| `reqwest` | HTTP 客户端 |
| `serde` / `serde_json` | JSON 序列化 |
| `serde_yaml` | YAML 配置文件解析 |
| `clap` | CLI 参数解析 |
| `chrono` | 时间日期处理 |
| `lz4_flex` | LZ4 压缩/解压 |
| `md-5` | MD5 哈希 (token ID) |
| `rand` | 随机数 (加密) |
| `base64` | Base64 编解码 |
| `qrcode` | 终端二维码生成 |
| `rqrr` | 二维码图片识别 |
| `image` | 图片解码 |
