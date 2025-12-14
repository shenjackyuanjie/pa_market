//! Worker节点 - 分布式ID扫描系统的边缘节点
//!
//! 功能：
//! - 持续循环获取任务
//! - 后台心跳保活
//! - HTTP探测（并发控制）
//! - 提交结果
//! - 优雅退出（ctrl+c）

use clap::Parser;
use common::{
    AcquireTaskRequest, AcquireTaskResponse, ApiResponse, HeartbeatRequest, ReleaseTaskRequest,
    SubmitResultRequest,
};
use futures::StreamExt;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::sleep;
use tracing::{error, info, warn};

/// Worker配置
#[derive(Parser, Debug, Clone)]
#[command(author, version, about = "分布式ID扫描系统 - Worker节点", long_about = None)]
struct Config {
    /// Master节点地址
    #[arg(short = 'm', long, default_value = "http://localhost:3000")]
    pub master_url: String,

    /// 初始处理速度（req/s）
    #[arg(short = 's', long, default_value = "100")]
    pub initial_speed: u32,

    /// HTTP并发数
    #[arg(short = 'c', long, default_value = "50")]
    pub concurrency: usize,

    /// 心跳间隔（秒）
    #[arg(short = 'b', long, default_value = "10")]
    pub heartbeat_interval: u64,

    /// 失败重试间隔（秒）
    #[arg(short = 'r', long, default_value = "5")]
    pub retry_interval: u64,
}

/// Worker状态
#[derive(Clone)]
struct WorkerState {
    /// Worker唯一标识符
    pub worker_id: String,

    /// 当前处理速度
    pub current_speed: Arc<RwLock<u32>>,

    /// HTTP客户端
    pub client: reqwest::Client,

    /// 是否收到退出信号（第一次 ctrl+c）
    pub shutdown_requested: Arc<AtomicBool>,

    /// 是否需要强制退出（第二次 ctrl+c）
    pub force_shutdown: Arc<AtomicBool>,

    /// 当前正在执行的任务ID（0表示没有任务）
    pub current_task_id: Arc<AtomicI32>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // 解析命令行参数
    let config = Config::parse();

    // 生成Worker ID
    let worker_id = uuid::Uuid::new_v4().to_string();
    info!("启动Worker节点，ID: {}", worker_id);
    info!("Master地址: {}", config.master_url);
    info!("初始速度: {} req/s", config.initial_speed);
    info!("并发数: {}", config.concurrency);

    // 创建Worker状态
    let state = Arc::new(WorkerState {
        worker_id: worker_id.clone(),
        current_speed: Arc::new(RwLock::new(config.initial_speed)),
        client: reqwest::Client::new(),
        shutdown_requested: Arc::new(AtomicBool::new(false)),
        force_shutdown: Arc::new(AtomicBool::new(false)),
        current_task_id: Arc::new(AtomicI32::new(0)),
    });

    // 设置 ctrl+c 信号处理
    let state_for_signal = Arc::clone(&state);
    let config_for_signal = config.clone();
    tokio::spawn(async move {
        setup_signal_handler(&config_for_signal, &state_for_signal).await;
    });

    // 启动主循环
    loop {
        // 检查是否收到退出信号
        if state.shutdown_requested.load(Ordering::SeqCst) {
            info!("收到退出信号，停止获取新任务");
            break;
        }

        match run_worker_loop(&config, &state).await {
            Ok(_) => {
                info!("任务完成，等待下一个任务...");
                sleep(Duration::from_secs(1)).await;
            }
            Err(e) => {
                error!(
                    "Worker循环错误: {}，在 {} 秒后重试...",
                    e, config.retry_interval
                );
                sleep(Duration::from_secs(config.retry_interval)).await;
            }
        }
    }

    info!("Worker已优雅退出");
    Ok(())
}

/// 设置信号处理器
async fn setup_signal_handler(config: &Config, state: &Arc<WorkerState>) {
    let mut first_signal = true;

    loop {
        tokio::signal::ctrl_c().await.expect("无法监听ctrl+c信号");

        if first_signal {
            first_signal = false;
            info!("收到第一次 ctrl+c，准备优雅退出...");
            info!("再次按 ctrl+c 将强制退出并释放当前任务");
            state.shutdown_requested.store(true, Ordering::SeqCst);
        } else {
            warn!("收到第二次 ctrl+c，强制退出！");
            state.force_shutdown.store(true, Ordering::SeqCst);

            // 释放当前任务
            let task_id = state.current_task_id.load(Ordering::SeqCst);
            if task_id > 0 {
                info!("正在释放任务 {}...", task_id);
                if let Err(e) = release_task(config, state, task_id).await {
                    error!("释放任务失败: {}", e);
                } else {
                    info!("任务 {} 已释放", task_id);
                }
            }

            std::process::exit(1);
        }
    }
}

/// 向Master释放任务
async fn release_task(
    config: &Config,
    state: &Arc<WorkerState>,
    task_id: i32,
) -> Result<(), Box<dyn std::error::Error>> {
    let request = ReleaseTaskRequest {
        task_id,
        worker_id: state.worker_id.clone(),
    };

    let url = format!("{}/task/release", config.master_url);
    let response: ApiResponse<String> = state
        .client
        .post(&url)
        .json(&request)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if !response.success {
        return Err(response
            .error
            .unwrap_or_else(|| "未知错误".to_string())
            .into());
    }

    Ok(())
}

/// Worker主循环
async fn run_worker_loop(
    config: &Config,
    state: &Arc<WorkerState>,
) -> Result<(), Box<dyn std::error::Error>> {
    // 1. 获取任务
    let task = acquire_task(config, state).await?;
    info!(
        "任务已获取: task_id={}, 范围=[{}, {}]",
        task.task_id, task.start_id, task.end_id
    );

    // 记录当前任务ID
    state.current_task_id.store(task.task_id, Ordering::SeqCst);

    // 2. 启动后台心跳任务
    let heartbeat_handle = {
        let config = config.clone();
        let state = Arc::clone(state);
        let task_id = task.task_id;

        tokio::spawn(async move {
            heartbeat_loop(&config, &state, task_id).await;
        })
    };

    // 3. 执行任务
    let start_time = Instant::now();
    let valid_ids = execute_task(config, state, &task).await?;
    let elapsed = start_time.elapsed();

    // 4. 停止心跳任务
    heartbeat_handle.abort();

    // 检查是否被强制退出
    if state.force_shutdown.load(Ordering::SeqCst) {
        return Err("强制退出".into());
    }

    // 5. 计算并更新处理速度
    let total_ids = (task.end_id - task.start_id + 1) as u32;
    let new_speed = if elapsed.as_secs() > 0 {
        total_ids / elapsed.as_secs() as u32
    } else {
        total_ids
    };

    {
        let mut speed = state.current_speed.write().await;
        *speed = new_speed;
    }

    info!(
        "任务完成: task_id={}, 总ID数={}, 有效ID数={}, 耗时={:.2}s, 速度={} req/s",
        task.task_id,
        total_ids,
        valid_ids.len(),
        elapsed.as_secs_f32(),
        new_speed
    );

    // 6. 提交结果
    submit_result(config, state, task.task_id, valid_ids).await?;

    // 清除当前任务ID
    state.current_task_id.store(0, Ordering::SeqCst);

    Ok(())
}

/// 从Master获取任务
async fn acquire_task(
    config: &Config,
    state: &Arc<WorkerState>,
) -> Result<AcquireTaskResponse, Box<dyn std::error::Error>> {
    // 获取当前处理速度
    let current_speed = *state.current_speed.read().await;

    let request = AcquireTaskRequest {
        worker_id: state.worker_id.clone(),
        last_performance: Some(current_speed),
    };

    let url = format!("{}/task/acquire", config.master_url);
    let response: ApiResponse<AcquireTaskResponse> = state
        .client
        .post(&url)
        .json(&request)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if !response.success {
        return Err(response
            .error
            .unwrap_or_else(|| "未知错误".to_string())
            .into());
    }

    response.data.ok_or_else(|| "没有任务数据".into())
}

/// 后台心跳循环
async fn heartbeat_loop(config: &Config, state: &Arc<WorkerState>, task_id: i32) {
    let interval = Duration::from_secs(config.heartbeat_interval);

    loop {
        sleep(interval).await;

        let request = HeartbeatRequest {
            task_id,
            worker_id: state.worker_id.clone(),
        };

        let url = format!("{}/task/heartbeat", config.master_url);
        match state.client.post(&url).json(&request).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    info!("任务 {} 的心跳已发送", task_id);
                } else {
                    warn!("心跳发送失败: status={}", resp.status());
                }
            }
            Err(e) => {
                warn!("心跳请求错误: {}", e);
            }
        }
    }
}


/// 检查ID是否有效
/// 返回值：
/// - `Some(true)` - ID 有效
/// - `Some(false)` - ID 无效
/// - `None` - appId 不匹配，需要重试
pub async fn check_id(client: &reqwest::Client, id: i64) -> Option<bool> {
    let app_id = format!("C{}", id);
    let body = serde_json::json!({
        "appId": app_id,
        "locale": "zh_CN",
        "countryCode": "CN",
        "orderApp": 1
    });

    let token = common::code::GLOBAL_CODE_MANAGER.get_full_token().await;
    let response = client
        .post("https://web-drcn.hispace.dbankcloud.com/edge/webedge/appinfo")
        .header("Content-Type", "application/json")
        .header("User-Agent", common::code::USER_AGENT.to_string())
        .header("interface-code", token.interface_code)
        .header("identity-id", token.identity_id)
        .json(&body)
        .send()
        .await;

    match response {
        Ok(resp) => {
            if resp.content_length().unwrap_or(0) == 0 {
                return Some(false);
            }
            if let Ok(value) = resp.json::<serde_json::Value>().await {
                if !value.is_object() {
                    return Some(false);
                }
                let value = value.as_object().unwrap();
                if !value.contains_key("appId") {
                    return Some(false);
                }
                let response_app_id = value
                    .get("appId")
                    .and_then(|v| v.as_str());
                match response_app_id {
                    Some(v) if v == app_id => Some(true),
                    Some(_) => None, // appId 不匹配，需要重试
                    None => Some(false),
                }
            } else {
                Some(false)
            }
        }
        Err(_) => Some(false),
    }
}

/// 执行扫描任务
async fn execute_task(
    config: &Config,
    state: &Arc<WorkerState>,
    task: &AcquireTaskResponse,
) -> Result<Vec<i64>, Box<dyn std::error::Error>> {
    let client = &state.client;
    let force_shutdown = Arc::clone(&state.force_shutdown);

    // 创建ID流
    let id_stream = futures::stream::iter(task.start_id..=task.end_id)
        .map(|id| {
            let client = client.clone();
            let force_shutdown = Arc::clone(&force_shutdown);
            async move {
                // 检查是否需要强制退出
                if force_shutdown.load(Ordering::SeqCst) {
                    return None;
                }

                // 重试逻辑：当 check_id 返回 None 时重试
                loop {
                    // 再次检查强制退出标志
                    if force_shutdown.load(Ordering::SeqCst) {
                        return None;
                    }

                    match check_id(&client, id).await {
                        Some(true) => {
                            info!("发现有效ID: {}", id);
                            return Some(id);
                        }
                        Some(false) => {
                            return None;
                        }
                        None => {
                            // appId 不匹配，需要重试
                            warn!("ID {} 检查时 appId 不匹配，重试中...", id);
                            continue;
                        }
                    }
                }
            }
        })
        .buffer_unordered(config.concurrency);

    // 收集有效ID
    let valid_ids: Vec<i64> = id_stream.filter_map(|x| async move { x }).collect().await;

    Ok(valid_ids)
}

/// 向Master提交结果
async fn submit_result(
    config: &Config,
    state: &Arc<WorkerState>,
    task_id: i32,
    valid_ids: Vec<i64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let request = SubmitResultRequest { task_id, valid_ids };

    let url = format!("{}/task/submit", config.master_url);
    let response: ApiResponse<String> = state
        .client
        .post(&url)
        .json(&request)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    if !response.success {
        return Err(response
            .error
            .unwrap_or_else(|| "未知错误".to_string())
            .into());
    }

    info!("任务 {} 提交成功", task_id);
    Ok(())
}
