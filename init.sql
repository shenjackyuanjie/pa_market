-- 分布式ID扫描系统数据库初始化脚本
-- SQLite

-- 1. global_cursor表: 存储全局任务分配进度
CREATE TABLE IF NOT EXISTS global_cursor (
    id INTEGER PRIMARY KEY,
    next_start_id INTEGER NOT NULL
);

-- 初始化数据: 从ID 0开始
INSERT OR IGNORE INTO global_cursor (id, next_start_id)
VALUES (1, 0);

-- 2. task_queue表: 存储正在运行或超时未完成的任务
CREATE TABLE IF NOT EXISTS task_queue (
    task_id INTEGER PRIMARY KEY AUTOINCREMENT,
    start_id INTEGER NOT NULL,
    end_id INTEGER NOT NULL,
    worker_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'running',
    last_heartbeat DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- 在last_heartbeat上创建索引，用于快速查找超时任务
CREATE INDEX IF NOT EXISTS idx_task_queue_last_heartbeat ON task_queue(last_heartbeat);
CREATE INDEX IF NOT EXISTS idx_task_queue_status ON task_queue(status);

-- 3. valid_results表: 存储扫描到的有效ID
CREATE TABLE IF NOT EXISTS valid_results (
    id INTEGER PRIMARY KEY,
    found_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- 在found_at上创建索引，便于按时间查询
CREATE INDEX IF NOT EXISTS idx_valid_results_found_at ON valid_results(found_at);