# Master 节点快速开始指南

## 🚀 30秒快速启动

### 第一次使用

```bash
# 1. 编译项目（第一次会比较慢）
cargo build --release

# 2. 启动 Master 服务
cargo run --release --bin master
```

**就这样！** 数据库文件会自动创建在 `master.db`

### 查看系统状态

在另一个终端运行：

```bash
cargo run --release --bin init -- status
```

输出示例：
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

## 📋 常见需求

### 从特定 ID 开始扫描

```bash
# 设置起始 ID 为 1000000
cargo run --release --bin init -- set-cursor 1000000

# 查看确认
cargo run --release --bin init -- status

# 启动 Master
cargo run --release --bin master
```

### 清空任务队列（保留结果）

```bash
cargo run --release --bin init -- reset-queue
```

### 完全重置系统

```bash
# 清空所有数据
cargo run --release --bin init -- clear --force

# 重新启动
cargo run --release --bin master
```

### 指定自定义数据库位置

```bash
# Master 使用自定义数据库
cargo run --release --bin master -- -d ./data/my_database.db

# 初始化工具也使用相同路径
cargo run --release --bin init -- -d ./data/my_database.db status
```

### 指定监听端口

```bash
cargo run --release --bin master -- -p 3001
```

### 组合使用

```bash
# 自定义路径 + 自定义端口 + 自定义主机
cargo run --release --bin master -- \
  -d ./databases/prod.db \
  -p 3001 \
  -H 127.0.0.1
```

## 🔧 可用命令

| 命令 | 说明 |
|------|------|
| `cargo run --release --bin master` | 启动 Master 服务（自动创建数据库） |
| `cargo run --release --bin init -- status` | 查看系统状态 |
| `cargo run --release --bin init -- set-cursor <ID>` | 设置扫描起始 ID |
| `cargo run --release --bin init -- reset-queue` | 清空未完成任务 |
| `cargo run --release --bin init -- clear --force` | 完全重置系统 |

## 💾 数据库

- **位置**: 默认为 `master.db`（当前目录）
- **格式**: SQLite 3
- **自动创建**: 是（首次运行时自动创建）
- **备份**: `cp master.db master.db.backup`
- **恢复**: `cp master.db.backup master.db`

## ⚙️ 选项参数

```bash
Master 节点选项:
  -d, --database-url <PATH>   数据库文件路径 [default: master.db]
  -H, --host <HOST>           监听地址 [default: 0.0.0.0]
  -p, --port <PORT>           监听端口 [default: 3000]

初始化工具选项:
  -d, --database-url <PATH>   数据库文件路径 [default: master.db]
```

## 🌐 API 端点

启动后，Master 在 `http://localhost:3000` 提供以下 API：

- `POST /task/acquire` - Worker 申请任务
- `POST /task/heartbeat` - Worker 发送心跳
- `POST /task/submit` - Worker 提交结果

## 📚 更多信息

- 详细配置: [DATABASE_MIGRATION.md](./DATABASE_MIGRATION.md)
- 完整指南: [INIT_GUIDE.md](./INIT_GUIDE.md)

## ✅ 验证安装

```bash
# 查看帮助信息
cargo run --release --bin master -- --help

# 查看初始化工具帮助
cargo run --release --bin init -- --help
```

## 常见问题

**Q: 启动时数据库文件没有创建？**
A: 程序会自动创建，确保有当前目录的写权限

**Q: 想用不同的数据库？**
A: 使用 `-d` 参数指定路径即可，创建自动进行

**Q: 多个 Master 实例如何共存？**
A: 为每个实例指定不同的数据库文件和端口：
```bash
# 实例 1
cargo run --release --bin master -- -d master1.db -p 3000

# 实例 2
cargo run --release --bin master -- -d master2.db -p 3001
```

**Q: 如何查询已扫描的结果？**
A: 直接查询 SQLite 数据库：
```bash
sqlite3 master.db "SELECT * FROM valid_results LIMIT 10;"
```
