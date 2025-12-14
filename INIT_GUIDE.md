# Master 节点初始化指南

## 概述

Master 节点提供了一个专用的初始化工具 `init`，用于管理任务队列的初始化、重置和状态查询。

## 快速开始

### 1. 初始化数据库

在首次运行之前，初始化数据库（创建表和索引）：

```bash
cargo run --bin init -- init-db
```

**说明**：
- 自动创建所有必要的表：`global_cursor`、`task_queue`、`valid_results`
- 设置全局游标初始值为 0
- 创建必要的索引以优化查询性能
- 如果表已存在，不会重复创建（幂等操作）

### 2. 设置扫描起始 ID

如果需要从特定的 ID 开始扫描，而不是从 0 开始：

```bash
cargo run --bin init -- set-cursor 1000000
```

**说明**：
- 将全局游标设置为指定的起始 ID
- Worker 将从这个 ID 开始申请任务
- 可以随时修改，但不影响已分配的任务

### 3. 查看当前状态

查看任务队列和扫描结果的统计信息：

```bash
cargo run --bin init -- status
```

**输出示例**：
```
╔════════════════════════════════════════╗
║         Master 节点任务状态            ║
╠════════════════════════════════════════╣
║ 全局游标位置:  1000000                 ║
║ 总任务数:      15                      ║
║ 运行中的任务:  10                      ║
║ 已扫描结果:    250000                  ║
╚════════════════════════════════════════╝
```

## 高级操作

### 重置任务队列

清空所有待执行的任务（但保留已扫描的结果）：

```bash
cargo run --bin init -- reset-queue
```

**用途**：
- 当某些 Worker 因故障无法完成任务时
- 需要重新分配这些任务给其他 Worker
- 游标位置不变，结果数据保留

### 清空所有数据（谨慎操作）

⚠️ **危险操作**：清空所有数据，包括待执行任务和已扫描结果

```bash
cargo run --bin init -- clear --force
```

**说明**：
- 必须加上 `--force` 标志才能执行
- 没有 `--force` 标志的命令会拒绝执行
- 删除所有任务队列中的记录
- 删除所有已扫描的结果
- 重置游标为 0

**使用场景**：
- 完全重新开始扫描
- 清理测试数据
- 系统重置

## 完整工作流程

### 场景 1：第一次启动系统

```bash
# 步骤 1：初始化数据库
cargo run --bin init -- init-db

# 步骤 2：检查初始状态
cargo run --bin init -- status

# 步骤 3：启动 Master 服务器
cargo run --bin master

# 步骤 4：启动 Worker（在另一个终端）
cargo run --bin worker
```

### 场景 2：从特定 ID 开始扫描

```bash
# 步骤 1：初始化数据库
cargo run --bin init -- init-db

# 步骤 2：设置起始 ID（比如从 100 万开始）
cargo run --bin init -- set-cursor 1000000

# 步骤 3：验证设置
cargo run --bin init -- status

# 步骤 4：启动服务
cargo run --bin master
```

### 场景 3：处理任务失败后的重试

```bash
# 步骤 1：查看当前状态
cargo run --bin init -- status

# 步骤 2：重置失败的任务
cargo run --bin init -- reset-queue

# 步骤 3：重新启动 Worker 重试
cargo run --bin worker
```

### 场景 4：完全重新扫描

```bash
# 步骤 1：清空所有数据（确认这是想要的操作）
cargo run --bin init -- clear --force

# 步骤 2：初始化数据库
cargo run --bin init -- init-db

# 步骤 3：开始新的扫描
cargo run --bin master
```

## 自定义数据库路径

如果 Master 使用了非默认的数据库路径，初始化工具也需要指定相同的路径：

```bash
# Master 使用自定义数据库
cargo run --bin master -- -d sqlite:/path/to/custom.db

# 初始化工具也需要指定相同的路径
cargo run --bin init -- -d sqlite:/path/to/custom.db init-db
```

## 常见问题

### Q: 初始化工具和 Master 使用不同的数据库会怎样？

A: 它们会操作不同的数据库文件，导致不一致。务必确保：
```bash
# 查看 Master 启动时使用的数据库
cargo run --bin master -- --help

# 使用相同的路径
cargo run --bin init -- -d <同样的路径> <命令>
```

### Q: 可以在 Master 运行时执行初始化命令吗？

A: 可以，但建议避免。Master 可能会在同时读写数据库，可能导致：
- 锁定冲突
- 数据不一致
- 操作失败

**最佳实践**：在 Master 停止运行时执行初始化命令。

### Q: 如何备份数据？

A: SQLite 数据库就是一个文件，直接复制即可：

```bash
# 备份
cp master.db master.db.backup

# 恢复
cp master.db.backup master.db
```

### Q: 设置新的游标位置后，之前的任务会怎样？

A: 
- 已分配给 Worker 的任务不受影响
- 只有新的任务分配会从新游标位置开始
- 这对于恢复场景很有用（例如，前 100 万 ID 已完成，现在跳过它们）

## 数据库状态详解

执行 `status` 命令会显示：

| 字段 | 说明 | 用途 |
|-----|------|------|
| 全局游标位置 | 下一个待分配任务的起始 ID | 监控扫描进度 |
| 总任务数 | task_queue 中的所有任务 | 了解当前工作量 |
| 运行中的任务 | status = 'running' 的任务 | 判断系统是否活跃 |
| 已扫描结果 | valid_results 表中的记录数 | 衡量已完成的工作 |

## 后续步骤

初始化完成后：

1. **启动 Master 服务器**
   ```bash
   cargo run --bin master
   ```

2. **启动 Worker 进程**
   ```bash
   cargo run --bin worker
   ```

3. **监控扫描进度**
   ```bash
   # 定期检查状态
   cargo run --bin init -- status
   ```

4. **导出结果** （需要自己编写脚本）
   ```bash
   sqlite3 master.db "SELECT * FROM valid_results ORDER BY found_at DESC LIMIT 100;"
   ```

更多信息请参考 [DATABASE_MIGRATION.md](./DATABASE_MIGRATION.md)。