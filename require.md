# 分布式 ID 扫描系统需求文档 (Rust Edition)

## 1. 项目概述
构建一个高性能的分布式任务分发系统，用于遍历 massive `i64` ID 范围并检测有效性。
系统由 **Master（中心节点）** 和 **Worker（边缘节点）** 组成。
Master 负责状态管理和任务分发，Worker 负责具体的 HTTP 探测工作。

## 2. 技术栈约束
*   **语言**: Rust (Edition 2021/2024)
*   **数据库**: PostgreSQL
*   **ORM/Driver**: `sqlx` (Async, Postgres features)
*   **Web 框架 (Master)**: `Axum`
*   **HTTP 客户端 (Worker)**: `Reqwest`
*   **异步运行时**: `Tokio`
*   **序列化**: `Serde` + `Serde_json`
*   **OS 兼容性**: 代码必须跨平台（Windows/Linux/macOS），路径和系统调用需使用标准库处理。

---

## 3. 数据库设计 (PostgreSQL)

请编写 SQL 脚本初始化以下三张表。使用 `BIGINT` 存储 ID。

1.  **`global_cursor`**: 存储全局任务分配进度。
    *   字段: `id` (PK, int), `next_start_id` (BIGINT).
    *   初始化数据: `(1, 0)`.

2.  **`task_queue`**: 存储正在运行或超时未完成的任务。
    *   字段: `task_id` (PK, SERIAL), `start_id` (BIGINT), `end_id` (BIGINT), `worker_id` (VARCHAR), `status` (VARCHAR: 'running'), `last_heartbeat` (TIMESTAMP), `created_at` (TIMESTAMP).
    *   索引: 对 `last_heartbeat` 建索引。

3.  **`valid_results`**: 存储扫描到的有效 ID。
    *   字段: `id` (PK, BIGINT), `found_at` (TIMESTAMP).

---

## 4. Master 节点详细设计

### 4.1 配置
*   监听端口: `3000` (默认)
*   数据库 URL: cli 参数 `-d` 读取。

### 4.2 核心逻辑：动态分发
Master 需要实现一个智能分发算法：
*   **任务超时判定**: 如果当前时间 - `task_queue.last_heartbeat` > **60秒**，视为任务失败。
*   **优先重试**: 分发任务时，**优先**查找表中已超时的任务。
*   **新任务生成**: 如果没有超时任务，则锁定 `global_cursor` 表，根据 Worker 请求的 `batch_size` 切分一段新 ID 范围，并更新游标。

### 4.3 API 接口定义 (Axum)

#### A. 获取任务 `POST /task/acquire`
*   **Request**:
    ```json
    {
      "worker_id": "uuid-string",
      "last_performance": 500 // 选填，Worker上一次任务的每秒处理速度
    }
    ```
*   **Logic**:
    1.  根据 `last_performance` 计算本次分配数量 `batch_size`。
        *   公式: `size = last_performance * 30` (期望运行30秒)。
        *   约束: `1000 <= size <= 50000`。
    2.  检查是否有超时任务 -> 若有，分配超时任务并更新 `worker_id` 和 `last_heartbeat`。
    3.  若无，从 `global_cursor` 切分新范围，插入 `task_queue`。
*   **Response**:
    ```json
    {
      "task_id": 101,
      "start_id": 10000,
      "end_id": 12000
    }
    ```

#### B. 任务保活 `POST /task/heartbeat`
*   **Request**: `{"task_id": 101, "worker_id": "..."}`
*   **Logic**: 更新对应 Task 的 `last_heartbeat = NOW()`。

#### C. 提交结果 `POST /task/submit`
*   **Request**:
    ```json
    {
      "task_id": 101,
      "valid_ids": [10005, 10088] // 发现的有效ID列表
    }
    ```
*   **Logic**:
    1.  事务开启。
    2.  将 `valid_ids` 批量写入 `valid_results` 表 (使用 `ON CONFLICT DO NOTHING`)。
    3.  从 `task_queue` 中**删除**该 `task_id`。
    4.  事务提交。

---

## 5. Worker 节点详细设计

### 5.1 架构
Worker 是一个基于 `loop` 的持续运行程序。需生成唯一的 `worker_id` (UUID)。

### 5.2 核心流程
1.  **初始化**: 生成 Worker ID，设置初始 `current_speed = 100`。
2.  **获取任务**: POST Master `/task/acquire`。
3.  **启动保活 (Heartbeat)**:
    *   开启一个后台 Tokio Task，每 **10秒** 发送一次心跳请求。
    *   如果主任务完成或失败，该后台 Task 必须被终止 (Abort)。
4.  **执行扫描 (Core Business)**:
    *   **占位函数**: `async fn check_id(id: i64) -> bool`。
    *   **并发控制**: 使用 `futures::stream` + `buffer_unordered` 控制并发数（例如并发 50 个请求）。
    *   *注意*: 这里只需要模拟 HTTP 请求耗时即可，预留好位置让我填写真实爬虫代码。
5.  **计算速度**: 记录任务耗时，更新 `current_speed` 用于下一次请求。
6.  **提交**: POST Master `/task/submit`。
7.  **异常处理**: 任何网络请求失败，打印 Log 并休眠 5 秒后重试（不退出进程）。

---

## 6. 代码结构要求 (Workspace 模式)

请生成一个 Cargo Workspace，包含两个 crate：

```text
/distri-crawler
  ├── Cargo.toml (workspace definition)
  ├── /common (lib)
  │     └── src/lib.rs (定义共享的 Request/Response 结构体，便于序列化)
  ├── /master (bin)
  │     └── src/main.rs (Axum 服务，SQLx 逻辑)
  └── /worker (bin)
        └── src/main.rs (循环获取任务，爬虫逻辑)
```

## 7. 补充说明
*   Master 端的 `sqlx` 需要使用 `PgPoolOptions` 设置最大连接数，防止连接耗尽。
*   Worker 端的 `reqwest::Client` 应该在 `main` 以外被构建一次并复用。
*   代码中请包含详细的中文注释，解释核心步骤。
