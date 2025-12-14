# PostgreSQL 到 SQLite 迁移说明

## 概述

本项目的 Master 节点已从 PostgreSQL 迁移到 SQLite。这个文档描述了所有的修改内容。

## 修改内容

### 1. 依赖更新

**文件**: `master/Cargo.toml`

```toml
# 之前
sqlx = { version = "0.8", features = ["runtime-tokio-rustls", "postgres", "chrono"] }

# 之后
sqlx = { version = "0.8", features = ["runtime-tokio-rustls", "sqlite", "chrono"] }
```

### 2. 数据库初始化脚本

**文件**: `init.sql`

主要变更：
- `TIMESTAMP` → `DATETIME` 
- `BIGINT` → `INTEGER`
- `VARCHAR(n)` → `TEXT`
- `SERIAL` → `INTEGER PRIMARY KEY AUTOINCREMENT`
- `INSERT INTO ... ON CONFLICT DO NOTHING` → `INSERT OR IGNORE INTO`
- `NOW()` → `CURRENT_TIMESTAMP`
- 移除了 PostgreSQL 特有的注释语法

### 3. Master 节点代码修改

**文件**: `master/src/main.rs`

#### 3.1 数据库连接

```rust
# 之前
use sqlx::{FromRow, PgPool, postgres::PgPoolOptions};

// 连接方式
let pool = PgPoolOptions::new()
    .max_connections(20)
    .connect(&config.database_url)
    .await?;

# 之后
use sqlx::{FromRow, SqlitePool, sqlite::SqlitePoolOptions};

// 连接方式
let pool = SqlitePoolOptions::new()
    .max_connections(20)
    .connect(&config.database_url)
    .await?;

// 自动初始化数据库表
init_database(&pool).await?;
```

#### 3.2 默认数据库 URL

```rust
# 之前
#[arg(short = 'd', long, default_value = "postgres://postgres:password@localhost/distri_crawler")]

# 之后
#[arg(short = 'd', long, default_value = "sqlite:master.db")]
```

#### 3.3 SQL 参数绑定语法

PostgreSQL 使用 `$1, $2, ...` 语法，SQLite 使用 `?, ?, ...` 或 `?1, ?2, ...` 语法：

```rust
# 之前 (PostgreSQL)
sqlx::query("UPDATE task_queue SET last_heartbeat = NOW() WHERE task_id = $1 AND worker_id = $2")

# 之后 (SQLite)
sqlx::query("UPDATE task_queue SET last_heartbeat = CURRENT_TIMESTAMP WHERE task_id = ? AND worker_id = ?")
```

#### 3.4 时间函数

```sql
# 之前
NOW()              # PostgreSQL

# 之后
CURRENT_TIMESTAMP  # SQLite
```

#### 3.5 冲突处理

```sql
# 之前
INSERT INTO table (id) VALUES ($1) ON CONFLICT (id) DO NOTHING

# 之后
INSERT OR IGNORE INTO table (id) VALUES (?)
```

#### 3.6 锁定机制

SQLite 不支持 `FOR UPDATE SKIP LOCKED`，已移除：

```rust
# 之前
SELECT ... FROM task_queue WHERE ... FOR UPDATE SKIP LOCKED

# 之后
SELECT ... FROM task_queue WHERE ... LIMIT 1
# SQLite 通过事务隐式处理并发控制
```

## 运行方式

### 启动 Master 节点（使用默认 SQLite 数据库）

```bash
cargo run --bin master
```

### 启动 Master 节点（指定自定义 SQLite 数据库路径）

```bash
cargo run --bin master -- -d sqlite:/path/to/database.db
```

### 启动 Master 节点（指定端口）

```bash
cargo run --bin master -- -p 3001
```

## 数据库特性

### SQLite 优势

- ✅ 无需外部服务器，开箱即用
- ✅ 完整的 ACID 事务支持
- ✅ 性能足以支持分布式任务调度
- ✅ 易于备份和迁移（就是一个文件）

### SQLite 限制

- ⚠️ 并发写入性能不如 PostgreSQL
- ⚠️ 不支持 PostgreSQL 特有的高级特性
- ⚠️ 文件大小限制（理论上 140TB，实际受操作系统限制）

## 数据库架构

SQLite 数据库包含以下表结构：

### global_cursor 表
- 存储全局任务分配进度的游标

### task_queue 表
- 存储正在运行或超时未完成的任务
- 索引：`last_heartbeat`（用于超时检测）
- 索引：`status`（用于快速过滤）

### valid_results 表
- 存储扫描到的有效 ID 结果
- 索引：`found_at`（用于时间范围查询）

## 迁移检查清单

- [x] 更新依赖声明
- [x] 转换 SQL 初始化脚本
- [x] 更新连接池配置
- [x] 转换所有 SQL 查询语句
- [x] 更新参数绑定语法
- [x] 更新时间函数调用
- [x] 移除 PostgreSQL 特定语法
- [x] 添加自动初始化逻辑
- [x] 编译验证通过

## 故障排除

### 问题：数据库文件无法创建

**解决方案**：确保 master 进程有写权限的目录存在：

```bash
# 创建目录（如果不存在）
mkdir -p ./data

# 启动时指定数据库路径
cargo run --bin master -- -d sqlite:./data/master.db
```

### 问题：并发写入冲突

**原因**：SQLite 的并发写入能力有限

**解决方案**：
- 确保任务队列的清理频率合理
- 如果需要更高的并发，考虑迁移回 PostgreSQL

### 问题：事务锁定超时

**原因**：SQLite 默认锁定超时为 5 秒

**解决方案**：增加锁定超时时间或优化查询性能