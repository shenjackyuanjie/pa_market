-- 分布式ID扫描系统数据库初始化脚本
-- PostgreSQL

-- 1. global_cursor表: 存储全局任务分配进度
CREATE TABLE IF NOT EXISTS global_cursor (
    id INT PRIMARY KEY,
    next_start_id BIGINT NOT NULL
);

-- 初始化数据: 从ID 0开始
INSERT INTO global_cursor (id, next_start_id)
VALUES (1, 0)
ON CONFLICT (id) DO NOTHING;

-- 2. task_queue表: 存储正在运行或超时未完成的任务
CREATE TABLE IF NOT EXISTS task_queue (
    task_id SERIAL PRIMARY KEY,
    start_id BIGINT NOT NULL,
    end_id BIGINT NOT NULL,
    worker_id VARCHAR(255) NOT NULL,
    status VARCHAR(50) NOT NULL DEFAULT 'running',
    last_heartbeat TIMESTAMP NOT NULL DEFAULT NOW(),
    created_at TIMESTAMP NOT NULL DEFAULT NOW()
);

-- 在last_heartbeat上创建索引，用于快速查找超时任务
CREATE INDEX IF NOT EXISTS idx_task_queue_last_heartbeat ON task_queue(last_heartbeat);
CREATE INDEX IF NOT EXISTS idx_task_queue_status ON task_queue(status);

-- 3. valid_results表: 存储扫描到的有效ID
CREATE TABLE IF NOT EXISTS valid_results (
    id BIGINT PRIMARY KEY,
    found_at TIMESTAMP NOT NULL DEFAULT NOW()
);

-- 在found_at上创建索引，便于按时间查询
CREATE INDEX IF NOT EXISTS idx_valid_results_found_at ON valid_results(found_at);

COMMENT ON TABLE global_cursor IS '全局任务分配进度游标';
COMMENT ON TABLE task_queue IS '任务队列，存储正在运行或超时的任务';
COMMENT ON TABLE valid_results IS '存储扫描到的有效ID结果';
