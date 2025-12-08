//! Common library for distributed crawler
//! 定义Master和Worker之间共享的请求/响应结构体

use serde::{Deserialize, Serialize};

/// Worker向Master请求任务时的请求体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcquireTaskRequest {
    /// Worker的唯一标识符
    pub worker_id: String,

    /// Worker上一次任务的每秒处理速度（可选）
    /// 用于Master动态调整batch_size
    pub last_performance: Option<u32>,
}

/// Master向Worker返回任务时的响应体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcquireTaskResponse {
    /// 任务ID
    pub task_id: i32,

    /// 起始ID（包含）
    pub start_id: i64,

    /// 结束ID（包含）
    pub end_id: i64,
}

/// Worker向Master发送心跳的请求体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRequest {
    /// 任务ID
    pub task_id: i32,

    /// Worker的唯一标识符
    pub worker_id: String,
}

/// Worker向Master提交结果的请求体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitResultRequest {
    /// 任务ID
    pub task_id: i32,

    /// 发现的有效ID列表
    pub valid_ids: Vec<i64>,
}

/// Master向Worker返回的通用响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse<T> {
    /// 是否成功
    pub success: bool,

    /// 数据负载
    pub data: Option<T>,

    /// 错误信息（如果有）
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    /// 创建成功的响应
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    /// 创建失败的响应
    pub fn error(msg: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg),
        }
    }
}
