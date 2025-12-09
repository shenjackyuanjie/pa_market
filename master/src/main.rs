//! Master节点 - 分布式ID扫描系统的中心节点
//!
//! 功能：
//! - 状态管理和任务分发
//! - 提供RESTful API供Worker调用
//! - 智能任务分发算法（优先重试超时任务）

use axum::{
    extract::State,
    http::StatusCode,
    routing::{post},
    Router,
};
use chrono::{Duration, Utc};
use clap::Parser;
use common::{
    AcquireTaskRequest, AcquireTaskResponse, ApiResponse, HeartbeatRequest, SubmitResultRequest,
};
use sqlx::{FromRow, PgPool, postgres::PgPoolOptions};
use std::{net::SocketAddr, sync::Arc};
use tracing::{error, info, warn};
use tower_http::trace::TraceLayer;

/// Master节点配置
#[derive(Parser, Debug)]
#[command(author, version, about = "分布式ID扫描系统 - Master节点", long_about = None)]
struct Config {
    /// 数据库连接URL
    #[arg(short = 'd', long, default_value = "postgres://postgres:password@localhost/distri_crawler")]
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
    /// PostgreSQL数据库连接池
    db_pool: PgPool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // 解析命令行参数
    let config = Config::parse();
    info!("Starting Master node on port {}", config.port);
    info!("Database URL: {}", config.database_url);

    // 创建数据库连接池
    let pool = PgPoolOptions::new()
        .max_connections(20)
        .connect(&config.database_url)
        .await?;

    // 测试数据库连接
    sqlx::query("SELECT 1").fetch_one(&pool).await?;
    info!("Database connected successfully");

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
        .expect("Invalid host:port combination");
    info!("Master server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// 获取任务
/// POST /task/acquire
async fn acquire_task(
    State(state): State<Arc<AppState>>,
    axum::Json(req): axum::Json<AcquireTaskRequest>,
) -> (StatusCode, axum::Json<ApiResponse<AcquireTaskResponse>>) {
    info!("Worker {} acquiring task", req.worker_id);

    // 计算batch_size（基于last_performance）
    let batch_size = calculate_batch_size(req.last_performance);
    info!("Calculated batch_size: {}", batch_size);

    // 尝试获取任务（优先分配超时任务）
    match try_acquire_task(&state.db_pool, &req.worker_id, batch_size).await {
        Ok(Some(task)) => {
            info!("Task acquired: task_id={}, range=[{}, {}]", task.task_id, task.start_id, task.end_id);
            (
                StatusCode::OK,
                axum::Json(ApiResponse::success(task)),
            )
        }
        Ok(None) => {
            warn!("No task available");
            (
                StatusCode::OK,
                axum::Json(ApiResponse::error("No task available".to_string())),
            )
        }
        Err(e) => {
            error!("Failed to acquire task: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(ApiResponse::error(format!("Database error: {}", e))),
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
    info!("Heartbeat from worker {} for task {}", req.worker_id, req.task_id);

    // 更新心跳时间
    let result = sqlx::query(
        "UPDATE task_queue SET last_heartbeat = NOW() WHERE task_id = $1 AND worker_id = $2"
    )
    .bind(req.task_id)
    .bind(&req.worker_id)
    .execute(&state.db_pool)
    .await;

    match result {
        Ok(res) => {
            if res.rows_affected() > 0 {
                info!("Heartbeat updated for task {}", req.task_id);
                StatusCode::OK
            } else {
                warn!("Task {} not found or worker mismatch", req.task_id);
                StatusCode::NOT_FOUND
            }
        }
        Err(e) => {
            error!("Failed to update heartbeat: {}", e);
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
    info!("Worker submitting result for task {}, valid_ids: {}", req.task_id, req.valid_ids.len());

    // 使用事务：写入结果 + 删除任务
    let mut tx = match state.db_pool.begin().await {
        Ok(t) => t,
        Err(e) => {
            error!("Failed to start transaction: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(ApiResponse::error(format!("Transaction error: {}", e))),
            );
        }
    };

    // 1. 批量写入valid_ids
    if !req.valid_ids.is_empty() {
        for id in &req.valid_ids {
            // 使用ON CONFLICT DO NOTHING避免重复
            let result = sqlx::query(
                "INSERT INTO valid_results (id) VALUES ($1) ON CONFLICT (id) DO NOTHING"
            )
            .bind(id)
            .execute(&mut *tx)
            .await;

            if let Err(e) = result {
                error!("Failed to insert valid_id {}: {}", id, e);
                let _ = tx.rollback().await;
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(ApiResponse::error(format!("Insert error: {}", e))),
                );
            }
        }
    }

    // 2. 从task_queue删除任务
    let result = sqlx::query("DELETE FROM task_queue WHERE task_id = $1")
        .bind(req.task_id)
        .execute(&mut *tx)
        .await;

    if let Err(e) = result {
        error!("Failed to delete task {}: {}", req.task_id, e);
        let _ = tx.rollback().await;
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(ApiResponse::error(format!("Delete error: {}", e))),
        );
    }

    // 提交事务
    if let Err(e) = tx.commit().await {
        error!("Failed to commit transaction: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(ApiResponse::error(format!("Commit error: {}", e))),
        );
    }

    info!("Task {} submitted successfully, {} valid ids", req.task_id, req.valid_ids.len());
    (
        StatusCode::OK,
        axum::Json(ApiResponse::success("Task submitted successfully".to_string())),
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
    pool: &PgPool,
    worker_id: &str,
    batch_size: i64,
) -> Result<Option<AcquireTaskResponse>, sqlx::Error> {
    // 查找超时任务（60秒未更新心跳）
    let timeout_duration = Duration::seconds(60);
    let timeout_time = Utc::now() - timeout_duration;

    // 使用FOR UPDATE SKIP LOCKED避免多个Worker分配到同一任务
    let timeout_task = sqlx::query_as::<_, TaskRecord>(
        r#"
        SELECT task_id, start_id, end_id, worker_id, status, last_heartbeat, created_at
        FROM task_queue
        WHERE last_heartbeat < $1
        ORDER BY last_heartbeat ASC
        LIMIT 1
        FOR UPDATE SKIP LOCKED
        "#,
    )
    .bind(timeout_time)
    .fetch_optional(pool)
    .await?;

    // 如果找到超时任务，分配给当前Worker
    if let Some(task) = timeout_task {
        info!("Found timeout task {}, reassigning to worker {}", task.task_id, worker_id);

        // 更新任务的worker_id和heartbeat
        sqlx::query(
            "UPDATE task_queue SET worker_id = $1, last_heartbeat = NOW() WHERE task_id = $2"
        )
        .bind(worker_id)
        .bind(task.task_id)
        .execute(pool)
        .await?;

        return Ok(Some(AcquireTaskResponse {
            task_id: task.task_id,
            start_id: task.start_id,
            end_id: task.end_id,
        }));
    }

    // 没有超时任务，从global_cursor切分新范围
    acquire_new_task(pool, worker_id, batch_size).await
}

/// 从global_cursor切分新任务
async fn acquire_new_task(
    pool: &PgPool,
    worker_id: &str,
    batch_size: i64,
) -> Result<Option<AcquireTaskResponse>, sqlx::Error> {
    // 开启事务
    let mut tx = pool.begin().await?;

    // 锁定global_cursor行（FOR UPDATE）
    let cursor_row = sqlx::query_as::<_, CursorRecord>(
        "SELECT id, next_start_id FROM global_cursor WHERE id = 1 FOR UPDATE"
    )
    .fetch_one(&mut *tx)
    .await?;

    let start_id = cursor_row.next_start_id;
    let end_id = start_id + batch_size - 1; // 包含end_id

    // 更新global_cursor
    sqlx::query("UPDATE global_cursor SET next_start_id = $1 WHERE id = 1")
        .bind(end_id + 1)
        .execute(&mut *tx)
        .await?;

    // 插入新任务到task_queue
    let task_id: i32 = sqlx::query_scalar(
        r#"
        INSERT INTO task_queue (start_id, end_id, worker_id, status, last_heartbeat)
        VALUES ($1, $2, $3, 'running', NOW())
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

    info!("Created new task: task_id={}, range=[{}, {}]", task_id, start_id, end_id);

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
