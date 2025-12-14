//! Master节点 - 分布式ID扫描系统的中心节点
//!
//! 功能：
//! - 状态管理和任务分发
//! - 提供RESTful API供Worker调用
//! - 智能任务分发算法（优先重试超时任务）

use axum::{extract::State, http::StatusCode, routing::post, Router};
use chrono::Utc;
use clap::Parser;
use common::{
    AcquireTaskRequest, AcquireTaskResponse, ApiResponse, HeartbeatRequest, SubmitResultRequest,
};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    FromRow, SqlitePool,
};
use std::str::FromStr;
use std::{net::SocketAddr, sync::Arc};
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};

/// Master节点配置
#[derive(Parser, Debug)]
#[command(author, version, about = "分布式ID扫描系统 - Master节点", long_about = None)]
struct Config {
    /// 数据库文件路径
    #[arg(short = 'd', long, default_value = "master.db")]
    database_url: String,

    /// 监听地址
    #[arg(short = 'H', long, default_value = "0.0.0.0")]
    host: String,

    /// 监听端口
    #[arg(short = 'p', long, default_value = "3000")]
    port: u16,
}

/// 应用状态
#[derive(Clone)]
struct AppState {
    /// SQLite数据库连接池
    db_pool: SqlitePool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // 解析命令行参数
    let config = Config::parse();
    info!("启动Master节点，端口: {}", config.port);
    info!("数据库路径: {}", config.database_url);

    // 确保数据库文件的目录存在
    if let Some(parent) = std::path::Path::new(&config.database_url).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    // 创建数据库连接池（使用标准文件路径，自动创建文件）
    let database_url = format!("sqlite:{}", config.database_url);
    let connect_options = SqliteConnectOptions::from_str(&database_url)?.create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(20)
        .connect_with(connect_options)
        .await?;

    // 执行初始化SQL
    init_database(&pool).await?;

    // 测试数据库连接
    sqlx::query("SELECT 1").fetch_one(&pool).await?;
    info!("数据库连接成功");

    // 创建应用状态
    let state = Arc::new(AppState { db_pool: pool });

    // 构建路由
    let app = Router::new()
        .route("/task/acquire", post(acquire_task))
        .route("/task/heartbeat", post(heartbeat))
        .route("/task/submit", post(submit_result))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // 启动服务器
    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .expect("无效的主机:端口组合");
    info!("Master服务器监听在 http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// 初始化数据库表
async fn init_database(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    // 创建global_cursor表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS global_cursor (
            id INTEGER PRIMARY KEY,
            next_start_id INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    // 初始化全局游标
    let result =
        sqlx::query("INSERT OR IGNORE INTO global_cursor (id, next_start_id) VALUES (1, 0)")
            .execute(pool)
            .await;

    match result {
        Ok(_) => info!("全局游标已初始化"),
        Err(e) => info!("全局游标已存在或出错: {}", e),
    }

    // 创建task_queue表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS task_queue (
            task_id INTEGER PRIMARY KEY AUTOINCREMENT,
            start_id INTEGER NOT NULL,
            end_id INTEGER NOT NULL,
            worker_id TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'running',
            last_heartbeat DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        )",
    )
    .execute(pool)
    .await?;

    // 创建task_queue的索引
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_task_queue_last_heartbeat ON task_queue(last_heartbeat)",
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_task_queue_status ON task_queue(status)")
        .execute(pool)
        .await?;

    // 创建valid_results表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS valid_results (
            id INTEGER PRIMARY KEY,
            found_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        )",
    )
    .execute(pool)
    .await?;

    // 创建valid_results的索引
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_valid_results_found_at ON valid_results(found_at)")
        .execute(pool)
        .await?;

    Ok(())
}

/// 获取任务
/// POST /task/acquire
async fn acquire_task(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<AcquireTaskRequest>,
) -> (StatusCode, axum::Json<ApiResponse<AcquireTaskResponse>>) {
    info!("Worker {} 请求任务", req.worker_id);

    // 计算batch_size（基于last_performance）
    let batch_size = calculate_batch_size(req.last_performance);
    info!("计算得到的batch_size: {}", batch_size);

    // 尝试获取任务（优先分配超时任务）
    match try_acquire_task(&state.db_pool, &req.worker_id, batch_size).await {
        Ok(Some(task)) => {
            info!(
                "任务已分配: task_id={}, 范围=[{}, {}]",
                task.task_id, task.start_id, task.end_id
            );
            (StatusCode::OK, axum::Json(ApiResponse::success(task)))
        }
        Ok(None) => {
            warn!("没有可用的任务");
            (
                StatusCode::OK,
                axum::Json(ApiResponse::error("没有可用的任务".to_string())),
            )
        }
        Err(e) => {
            error!("获取任务失败: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(ApiResponse::error(format!("数据库错误: {}", e))),
            )
        }
    }
}

/// 任务保活
/// POST /task/heartbeat
async fn heartbeat(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<HeartbeatRequest>,
) -> StatusCode {
    info!(
        "收到来自worker {} 的任务 {} 的心跳",
        req.worker_id, req.task_id
    );

    // 先查询任务当前状态，用于调试
    let task_info: Option<(String, String)> = sqlx::query_as(
        "SELECT worker_id, last_heartbeat FROM task_queue WHERE task_id = ?"
    )
    .bind(req.task_id)
    .fetch_optional(&state.db_pool)
    .await
    .ok()
    .flatten();

    if let Some((current_worker_id, last_heartbeat)) = &task_info {
        info!(
            "任务 {} 当前状态: worker_id={}, last_heartbeat={}",
            req.task_id, current_worker_id, last_heartbeat
        );
        if current_worker_id != &req.worker_id {
            warn!(
                "Worker ID不匹配! 请求的worker_id={}, 数据库中的worker_id={}",
                req.worker_id, current_worker_id
            );
        }
    } else {
        warn!("任务 {} 在数据库中不存在", req.task_id);
    }

    // 更新心跳时间
    let result = sqlx::query(
        "UPDATE task_queue SET last_heartbeat = datetime('now') WHERE task_id = ? AND worker_id = ?"
    )
    .bind(req.task_id)
    .bind(&req.worker_id)
    .execute(&state.db_pool)
    .await;

    match result {
        Ok(res) => {
            if res.rows_affected() > 0 {
                info!("任务 {} 的心跳已更新", req.task_id);
                StatusCode::OK
            } else {
                warn!("任务 {} 不存在或Worker不匹配 (rows_affected=0)", req.task_id);
                StatusCode::NOT_FOUND
            }
        }
        Err(e) => {
            error!("更新心跳失败: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

/// 提交结果
/// POST /task/submit
async fn submit_result(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<SubmitResultRequest>,
) -> (StatusCode, axum::Json<ApiResponse<String>>) {
    info!(
        "Worker提交任务 {} 的结果，发现有效ID数: {}",
        req.task_id,
        req.valid_ids.len()
    );

    // 使用事务：写入结果 + 删除任务
    let mut tx = match state.db_pool.begin().await {
        Ok(t) => t,
        Err(e) => {
            error!("启动事务失败: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(ApiResponse::error(format!("事务错误: {}", e))),
            );
        }
    };

    // 1. 批量写入valid_ids
    if !req.valid_ids.is_empty() {
        for id in &req.valid_ids {
            // 使用INSERT OR IGNORE避免重复
            let result = sqlx::query("INSERT OR IGNORE INTO valid_results (id) VALUES (?)")
                .bind(id)
                .execute(&mut *tx)
                .await;

            if let Err(e) = result {
                error!("插入有效ID {} 失败: {}", id, e);
                let _ = tx.rollback().await;
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(ApiResponse::error(format!("插入错误: {}", e))),
                );
            }
        }
    }

    // 2. 从task_queue删除任务
    let result = sqlx::query("DELETE FROM task_queue WHERE task_id = ?")
        .bind(req.task_id)
        .execute(&mut *tx)
        .await;

    if let Err(e) = result {
        error!("删除任务 {} 失败: {}", req.task_id, e);
        let _ = tx.rollback().await;
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(ApiResponse::error(format!("删除错误: {}", e))),
        );
    }

    // 提交事务
    if let Err(e) = tx.commit().await {
        error!("提交事务失败: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(ApiResponse::error(format!("提交错误: {}", e))),
        );
    }

    info!(
        "任务 {} 提交成功，发现 {} 个有效ID",
        req.task_id,
        req.valid_ids.len()
    );
    (
        StatusCode::OK,
        axum::Json(ApiResponse::success("任务提交成功".to_string())),
    )
}

/// 计算batch_size（基于last_performance）
/// 公式: size = last_performance * 30 (期望运行30秒)
/// 约束: 1000 <= size <= 50000
fn calculate_batch_size(last_performance: Option<u32>) -> i64 {
    const MIN_SIZE: i64 = 1000;
    const MAX_SIZE: i64 = 50000;
    const EXPECTED_RUNTIME_SECS: i64 = 30;

    let base_speed = last_performance.unwrap_or(100) as i64; // 默认100 req/s
    let size = base_speed * EXPECTED_RUNTIME_SECS;

    size.clamp(MIN_SIZE, MAX_SIZE)
}

/// 尝试获取任务
/// 1. 优先查找超时任务（last_heartbeat > 60秒）
/// 2. 如果没有超时任务，从global_cursor切分新范围
async fn try_acquire_task(
    pool: &SqlitePool,
    worker_id: &str,
    batch_size: i64,
) -> Result<Option<AcquireTaskResponse>, sqlx::Error> {
    // 开启事务，确保 FOR UPDATE SKIP LOCKED 能正常工作
    let mut tx = pool.begin().await?;

    // 查找超时任务（60秒未更新心跳）
    // 使用SQLite内置函数datetime计算超时时间，确保时间格式一致
    // CURRENT_TIMESTAMP和datetime都使用SQLite的UTC时间
    let timeout_task = sqlx::query_as::<_, TaskRecord>(
        r#"
        SELECT task_id, start_id, end_id, worker_id, status, last_heartbeat, created_at
        FROM task_queue
        WHERE last_heartbeat < datetime('now', '-60 seconds')
        ORDER BY last_heartbeat ASC
        LIMIT 1
        "#,
    )
    .fetch_optional(&mut *tx)
    .await?;

    // 如果找到超时任务，分配给当前Worker
    if let Some(task) = timeout_task {
        warn!(
            "发现超时任务 {}: 原worker={}, last_heartbeat={}, 现在重新分配给worker {}",
            task.task_id, task.worker_id, task.last_heartbeat, worker_id
        );

        // 更新任务的worker_id和heartbeat
        sqlx::query(
            "UPDATE task_queue SET worker_id = ?, last_heartbeat = datetime('now') WHERE task_id = ?"
        )
        .bind(worker_id)
        .bind(task.task_id)
        .execute(&mut *tx)
        .await?;

        // 提交事务
        tx.commit().await?;

        return Ok(Some(AcquireTaskResponse {
            task_id: task.task_id,
            start_id: task.start_id,
            end_id: task.end_id,
        }));
    }

    // 没有超时任务，回滚事务（实际上没有任何修改，但需要结束事务）
    tx.rollback().await?;

    // 从global_cursor切分新范围
    acquire_new_task(pool, worker_id, batch_size).await
}

/// 从global_cursor切分新任务
async fn acquire_new_task(
    pool: &SqlitePool,
    worker_id: &str,
    batch_size: i64,
) -> Result<Option<AcquireTaskResponse>, sqlx::Error> {
    // 开启事务
    let mut tx = pool.begin().await?;

    // 锁定global_cursor行
    let cursor_row = sqlx::query_as::<_, CursorRecord>(
        "SELECT id, next_start_id FROM global_cursor WHERE id = 1",
    )
    .fetch_one(&mut *tx)
    .await?;

    let start_id = cursor_row.next_start_id;
    let end_id = start_id + batch_size - 1; // 包含end_id

    // 更新global_cursor
    sqlx::query("UPDATE global_cursor SET next_start_id = ? WHERE id = 1")
        .bind(end_id + 1)
        .execute(&mut *tx)
        .await?;

    // 插入新任务到task_queue
    let task_id: i32 = sqlx::query_scalar(
        r#"
        INSERT INTO task_queue (start_id, end_id, worker_id, status, last_heartbeat)
        VALUES (?, ?, ?, 'running', datetime('now'))
        RETURNING task_id
        "#,
    )
    .bind(start_id)
    .bind(end_id)
    .bind(worker_id)
    .fetch_one(&mut *tx)
    .await?;

    // 提交事务
    tx.commit().await?;

    info!(
        "创建新任务: task_id={}, 范围=[{}, {}]",
        task_id, start_id, end_id
    );

    Ok(Some(AcquireTaskResponse {
        task_id,
        start_id,
        end_id,
    }))
}

/// 游标记录
#[derive(FromRow)]
#[allow(dead_code)]
struct CursorRecord {
    id: i32,
    next_start_id: i64,
}

/// 任务记录
#[derive(FromRow)]
#[allow(dead_code)]
struct TaskRecord {
    task_id: i32,
    start_id: i64,
    end_id: i64,
    worker_id: String,
    status: String,
    last_heartbeat: chrono::DateTime<Utc>,
    created_at: chrono::DateTime<Utc>,
}
