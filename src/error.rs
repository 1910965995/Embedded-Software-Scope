use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("USB 错误: {0}")]
    Usb(#[from] rusb::Error),

    #[error("未找到 CMSIS-DAP v2 设备")]
    DeviceNotFound,

    #[error("非预期的 DAP 响应: {0}")]
    InvalidResponse(String),

    #[error("DAP Transfer 失败: 状态={0}, 完成数={1}")]
    TransferFailed(u8, u8),

    #[error("SWD 错误: {0}")]
    Swd(String),

    #[error("操作超时")]
    Timeout,

    #[error("流水线线程启动失败: {0}")]
    PipelineThread(String),

    #[error("流水线已停止")]
    PipelineStopped,

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
