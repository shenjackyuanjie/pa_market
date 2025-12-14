//! Master 节点初始化工具
//! 用于管理任务队列的初始化和重置

use clap::{Parser, Subcommand};
use sqlx::sqlite::{SqlitePoolOptions, SqliteConnectOptions};
use std::str::FromStr;
use tracing::info;

#[derive(Parser)]
#[command(
    name = "init",
    about = "Master 节点任务初始化工具",
    long_about = "用于初始化和管理 Master 节点的任务队列"
)]
struct Cli {
    /// 数据库文件路径（如：master.db 或 ./data/master.db）
    #[arg(short = 'd', long, default_value = "master.db")]
    database_url: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 初始化数据库（创建表）
    InitDb,
    
    /// 设置全局游标位置
    #[command(about = "设置扫描起始 ID")]
    SetCursor {
        /// 起始 ID
        #[arg(value_name = "START_ID")]
        start_id: i64,
    },

    /// 重置任务队列（清空所有待执行任务）
    ResetQueue,

    /// 显示当前状态
    Status,

    /// 清空所有数据（包括已完成的结果）
    #[command(about = "危险操作：清空所有数据")]
    Clear {
        /// 必须提供 --force 标志才能执行
        #[arg(long)]
        force: bool,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // 解析命令行参数
    let cli = Cli::parse();
    info!("Connecting to database: {}", cli.database_url);

    // 确保数据库文件的目录存在
    if let Some(parent) = std::path::Path::new(&cli.database_url).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    // 创建数据库连接池（使用标准文件路径，自动创建文件）
    let database_url = format!("sqlite:{}", cli.database_url);
    let connect_options = SqliteConnectOptions::from_str(&database_url)?
        .create_if_missing(true);
    
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(connect_options)
        .await?;

    // 测试连接
    sqlx::query("SELECT 1").fetch_one(&pool).await?;
    info!("Database connection successful");

    // 执行相应的命令
    match cli.command {
        Commands::InitDb => init_db(&pool).await?,
        Commands::SetCursor { start_id } => set_cursor(&pool, start_id).await?,
        Commands::ResetQueue => reset_queue(&pool).await?,
        Commands::Status => show_status(&pool).await?,
        Commands::Clear { force } => clear_all(&pool, force).await?,
    }

    Ok(())
}

/// 初始化数据库
async fn init_db(pool: &sqlx::SqlitePool) -> Result<(), Box<dyn std::error::Error>> {
    info!("Initializing database...");

    // 创建 global_cursor 表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS global_cursor (
            id INTEGER PRIMARY KEY,
            next_start_id INTEGER NOT NULL
        )"
    )
    .execute(pool)
    .await?;

    // 初始化全局游标
    sqlx::query(
        "INSERT OR IGNORE INTO global_cursor (id, next_start_id) VALUES (1, 0)"
    )
    .execute(pool)
    .await?;

    // 创建 task_queue 表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS task_queue (
            task_id INTEGER PRIMARY KEY AUTOINCREMENT,
            start_id INTEGER NOT NULL,
            end_id INTEGER NOT NULL,
            worker_id TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'running',
            last_heartbeat DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        )"
    )
    .execute(pool)
    .await?;

    // 创建索引
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_task_queue_last_heartbeat ON task_queue(last_heartbeat)"
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_task_queue_status ON task_queue(status)"
    )
    .execute(pool)
    .await?;

    // 创建 valid_results 表
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS valid_results (
            id INTEGER PRIMARY KEY,
            found_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        )"
    )
    .execute(pool)
    .await?;

    // 创建索引
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_valid_results_found_at ON valid_results(found_at)"
    )
    .execute(pool)
    .await?;

    info!("Database initialized successfully");
    Ok(())
}

/// 设置全局游标
async fn set_cursor(pool: &sqlx::SqlitePool, start_id: i64) -> Result<(), Box<dyn std::error::Error>> {
    info!("Setting cursor to: {}", start_id);

    sqlx::query("UPDATE global_cursor SET next_start_id = ? WHERE id = 1")
        .bind(start_id)
        .execute(pool)
        .await?;

    info!("✓ Cursor updated to {}", start_id);
    Ok(())
}

/// 重置任务队列
async fn reset_queue(pool: &sqlx::SqlitePool) -> Result<(), Box<dyn std::error::Error>> {
    info!("Resetting task queue...");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_queue")
        .fetch_one(pool)
        .await?;

    sqlx::query("DELETE FROM task_queue")
        .execute(pool)
        .await?;

    info!("✓ Task queue cleared ({} tasks deleted)", count);
    Ok(())
}

/// 显示当前状态
async fn show_status(pool: &sqlx::SqlitePool) -> Result<(), Box<dyn std::error::Error>> {
    // 获取游标位置
    let cursor: (i64,) = sqlx::query_as("SELECT next_start_id FROM global_cursor WHERE id = 1")
        .fetch_one(pool)
        .await?;

    // 获取任务队列统计
    let task_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM task_queue")
        .fetch_one(pool)
        .await?;

    let running_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM task_queue WHERE status = 'running'")
        .fetch_one(pool)
        .await?;

    // 获取已扫描的结果数
    let result_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM valid_results")
        .fetch_one(pool)
        .await?;

    println!("\n╔════════════════════════════════════════╗");
    println!("║         Master 节点任务状态            ║");
    println!("╠════════════════════════════════════════╣");
    println!("║ 全局游标位置:  {:<22} ║", cursor.0);
    println!("║ 总任务数:      {:<22} ║", task_count.0);
    println!("║ 运行中的任务:  {:<22} ║", running_count.0);
    println!("║ 已扫描结果:    {:<22} ║", result_count.0);
    println!("╚════════════════════════════════════════╝\n");

    Ok(())
}

/// 清空所有数据
async fn clear_all(pool: &sqlx::SqlitePool, force: bool) -> Result<(), Box<dyn std::error::Error>> {
    if !force {
        eprintln!("⚠️  危险操作：此操作将删除所有数据");
        eprintln!("使用 --force 标志确认执行此操作");
        std::process::exit(1);
    }

    info!("Clearing all data...");

    sqlx::query("DELETE FROM valid_results").execute(pool).await?;
    sqlx::query("DELETE FROM task_queue").execute(pool).await?;
    sqlx::query("UPDATE global_cursor SET next_start_id = 0 WHERE id = 1").execute(pool).await?;

    info!("✓ All data cleared successfully");
    Ok(())
}