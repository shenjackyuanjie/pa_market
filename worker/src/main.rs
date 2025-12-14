//! Worker节点 - 分布式ID扫描系统的边缘节点
//!
//! 功能：
//! - 持续循环获取任务
//! - 后台心跳保活
//! - HTTP探测（并发控制）
//! - 提交结果

use clap::Parser;
use common::{
    AcquireTaskRequest, AcquireTaskResponse, ApiResponse, HeartbeatRequest, SubmitResultRequest,
};
use futures::StreamExt;
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
    });

    // 启动主循环
    loop {
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
    let valid_ids = execute_task(config, &state.client, &task).await?;
    let elapsed = start_time.elapsed();

    // 4. 停止心跳任务
    heartbeat_handle.abort();

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

pub async fn get_app_data(client: &reqwest::Client, app_id: &str) -> bool {
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
                return false;
            }
            if let Ok(value) = resp.json::<serde_json::Value>().await {
                if !value.is_object() {
                    return false;
                }
                let value = value.as_object().unwrap();
                if !value.contains_key("appId") {
                    return false;
                }
                if !value
                    .get("appId")
                    .map(|v| v.as_str())
                    .flatten()
                    .map(|v| v == app_id)
                    .unwrap_or(false)
                {
                    return false;
                }
                true
            } else {
                false
            }
        }
        Err(_) => false,
    }
}

/// 执行扫描任务
async fn execute_task(
    config: &Config,
    client: &reqwest::Client,
    task: &AcquireTaskResponse,
) -> Result<Vec<i64>, Box<dyn std::error::Error>> {
    // 创建ID流
    let id_stream = futures::stream::iter(task.start_id..=task.end_id)
        .map(|id| {
            let client = client.clone();
            async move {
                let app_id = format!("C{}", id);
                let is_valid = get_app_data(&client, &app_id).await;
                if is_valid {
                    info!("发现有效ID: {}", id);
                    Some(id)
                } else {
                    None
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
