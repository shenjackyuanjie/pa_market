# 分布式ID扫描系统

高性能的分布式任务分发系统，用于遍历 massive `i64` ID 范围并检测有效性。

## 系统架构

系统由两个核心组件组成：
- **Master（中心节点）**：负责任务分发、状态管理和结果存储
- **Worker（边缘节点）**：负责具体的HTTP探测工作

## 技术栈

- **语言**: Rust (Edition 2021)
- **数据库**: PostgreSQL
- **Web框架**: Axum (Master) + Reqwest (Worker)
- **ORM/Driver**: SQLx (异步PostgreSQL驱动)
- **异步运行时**: Tokio
- **序列化**: Serde + Serde_json

## 项目结构

```
distri-crawler/
├── Cargo.toml              # Workspace定义
├── init.sql                # 数据库初始化脚本
├── README.md               # 本文档
├── common/                 # 共享库
│   ├── Cargo.toml
│   └── src/lib.rs         # 请求/响应结构体定义
├── master/                 # Master节点
│   ├── Cargo.toml
│   └── src/main.rs        # Axum服务 + SQLx逻辑
└── worker/                 # Worker节点
    ├── Cargo.toml
    └── src/main.rs        # 循环任务获取 + HTTP探测
```

## 快速开始

### 1. 数据库准备

```bash
# 安装PostgreSQL（如果尚未安装）
# 创建数据库
createdb distri_crawler

# 初始化表结构
psql -d distri_crawler -f init.sql
```

### 2. 编译项目

```bash
# 编译所有crate（初次编译可能需要较长时间）
cargo build --release
```

### 3. 启动Master节点

```bash
cd master
cargo run --release -- -d "postgres://postgres:password@localhost/distri_crawler" -p 3000
```

参数说明：
- `-d`: 数据库连接URL
- `-h`: 监听地址（默认: 0.0.0.0）
- `-p`: 监听端口（默认: 3000）

### 4. 启动Worker节点

```bash
cd worker
cargo run --release
```

Worker会自动：
- 生成唯一的Worker ID
- 连接Master获取任务
- 执行扫描并提交结果

可以启动多个Worker实例以提高扫描速度。

## 配置说明

### Worker配置

在 `worker/src/main.rs` 中修改 `Config` 结构体：

```rust
let config = Config {
    master_url: "http://localhost:3000".to_string(),  // Master地址
    initial_speed: 100,     // 初始速度（req/s）
    concurrency: 50,        // HTTP并发数
    heartbeat_interval: 10, // 心跳间隔（秒）
    retry_interval: 5,      // 失败重试间隔（秒）
};
```

## 核心特性

### Master节点

- **智能任务分发**：优先分配超时任务（60秒未更新心跳）
- **动态Batch Size**：根据Worker上报的性能动态调整任务大小
- **心跳检测**：自动检测失效Worker并重分配任务
- **事务保证**：结果写入和任务删除的原子性

### Worker节点

- **持续运行**：循环获取任务，永不退出
- **心跳保活**：后台线程定期发送心跳
- **并发控制**：使用 `futures::stream::buffer_unordered` 控制并发
- **速度自适应**：根据实际性能动态调整速度
- **异常恢复**：网络错误自动重试

## 数据库设计

### 1. global_cursor表
存储全局任务分配进度，确保ID范围不重复分配。

### 2. task_queue表
存储正在运行或超时的任务，用于故障恢复和重试。

### 3. valid_results表
存储扫描到的有效ID，使用 `ON CONFLICT DO NOTHING` 避免重复。

## 扩展开发

### 添加真实的HTTP探测逻辑

在 `worker/src/main.rs` 中修改 `check_id` 函数：

```rust
async fn check_id(id: i64, _config: &Config) -> Result<bool, Box<dyn std::error::Error>> {
    // 在此处填写真实的HTTP爬虫代码
    // 示例：
    let url = format!("https://api.example.com/items/{}", id);
    let response = reqwest::get(&url).await?;

    // 根据HTTP状态码或响应内容判断ID是否有效
    Ok(response.status().is_success())
}
```

## 性能调优

### Master节点
- 调整 `PgPoolOptions::max_connections()`（默认: 20）

### Worker节点
- 增加 `concurrency` 提高并发（建议: 50-200）
- 启动多个Worker进程
- 调整 `heartbeat_interval` 平衡网络开销和任务恢复速度

### PostgreSQL
- 为 `task_queue(last_heartbeat)` 创建索引（已自动创建）
- 监控连接数：`SELECT count(*) FROM pg_stat_activity;`

## 监控和日志

使用环境变量控制日志级别：

```bash
# 显示所有日志（包括Debug）
RUST_LOG=debug cargo run

# 显示Info及以上
RUST_LOG=info cargo run

# 仅显示Error
RUST_LOG=error cargo run
```

## 故障排查

### Master无法启动
- 检查数据库连接URL是否正确
- 确保PostgreSQL服务正在运行
- 确认数据库 `distri_crawler` 已创建

### Worker无法获取任务
- 检查Master地址是否正确
- 确认Master已启动并监听正确端口
- 检查网络连接： `curl http://localhost:3000/task/acquire`

### 任务频繁超时
- 检查Worker是否正常运行
- 增加 `heartbeat_interval` 或调整任务大小
- 查看Worker日志是否有错误

## 许可证

MIT License
