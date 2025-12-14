# SQLite 数据库自动创建修复说明

## 问题描述

原始代码在启动 Master 服务器时出现以下错误：

```
Error: Database(SqliteError { code: 14, message: "unable to open database file" })
```

这是因为 SQLite 连接池默认不会自动创建数据库文件。

## 解决方案

### 1. 修改 Master 服务器 (`master/src/main.rs`)

使用 `SqliteConnectOptions` 的 `create_if_missing(true)` 选项：

```rust
use sqlx::sqlite::{SqlitePoolOptions, SqliteConnectOptions};
use std::str::FromStr;

// 创建连接选项，启用自动创建
let database_url = format!("sqlite:{}", config.database_url);
let connect_options = SqliteConnectOptions::from_str(&database_url)?
    .create_if_missing(true);

// 使用选项创建连接池
let pool = SqlitePoolOptions::new()
    .max_connections(20)
    .connect_with(connect_options)
    .await?;
```

### 2. 修改初始化工具 (`master/src/bin/init.rs`)

同样的逻辑应用到初始化工具中：

```rust
use sqlx::sqlite::{SqlitePoolOptions, SqliteConnectOptions};
use std::str::FromStr;

// 创建连接选项
let database_url = format!("sqlite:{}", cli.database_url);
let connect_options = SqliteConnectOptions::from_str(&database_url)?
    .create_if_missing(true);

// 使用选项创建连接池
let pool = SqlitePoolOptions::new()
    .max_connections(5)
    .connect_with(connect_options)
    .await?;
```

### 3. 目录创建

确保数据库文件所在的目录存在（如果路径包含目录）：

```rust
// 确保目录存在
if let Some(parent) = std::path::Path::new(&config.database_url).parent() {
    if !parent.as_os_str().is_empty() {
        std::fs::create_dir_all(parent)?;
    }
}
```

## 数据库 URL 格式变更

### 之前
```
数据库 URL: sqlite:master.db
```

### 之后
```
数据库文件路径: master.db
内部转换为: sqlite:master.db
```

这样更直观，用户只需提供文件路径，不需要关心 SQLite 的 URL 格式。

## 使用变化

### 启动 Master（自动创建数据库）

```bash
# 简单方式 - 在当前目录创建 master.db
cargo run --release --bin master

# 指定自定义路径
cargo run --release --bin master -- -d ./data/scan.db
```

### 初始化工具

```bash
# 查看状态（自动创建数据库）
cargo run --release --bin init -- status

# 指定自定义路径
cargo run --release --bin init -- -d ./data/scan.db status
```

## 关键改进

1. **自动创建数据库文件** - 无需手动创建或初始化
2. **自动创建目录** - 如果数据库路径包含不存在的目录，会自动创建
3. **路径友好** - 用户提供普通文件路径，不需要了解 SQLite URL 格式
4. **一致性** - Master 和初始化工具使用相同的逻辑

## 验证修复

执行以下命令验证数据库自动创建成功：

```bash
# 启动 Master（会创建 master.db）
cargo run --release --bin master &

# 等待几秒，然后查看文件
ls -lah master.db

# 查看状态
cargo run --release --bin init -- status
```

如果看到类似输出说明修复成功：

```
╔════════════════════════════════════════╗
║         Master 节点任务状态            ║
╠════════════════════════════════════════╣
║ 全局游标位置:  0                      ║
║ 总任务数:      0                      ║
║ 运行中的任务:  0                      ║
║ 已扫描结果:    0                      ║
╚════════════════════════════════════════╝
```

## 相关文件修改

- `pa_market/master/Cargo.toml` - 添加 init 二进制文件
- `pa_market/master/src/main.rs` - 修改数据库连接逻辑
- `pa_market/master/src/bin/init.rs` - 修改初始化工具的数据库连接逻辑
- `pa_market/QUICKSTART.md` - 新增快速开始指南