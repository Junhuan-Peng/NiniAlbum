use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("尚未打开项目")]
    NoProject,
    #[error("项目数据库已存在：{0}")]
    ProjectExists(PathBuf),
    #[error("照片目录不存在或不可访问：{0}")]
    InvalidPhotoRoot(PathBuf),
    #[error("路径无法转换为项目内的相对路径：{0}")]
    InvalidRelativePath(PathBuf),
    #[error("标签名称不能为空")]
    EmptyTagName,
    #[error("标签名称包含跨平台不安全字符或保留名称：{0}")]
    InvalidTagName(String),
    #[error("标签正在被 {0} 张照片使用")]
    TagInUse(i64),
    #[error("没有找到记录：{0}")]
    NotFound(String),
    #[error("数据库错误：{0}")]
    Database(#[from] rusqlite::Error),
    #[error("文件操作失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("图片处理失败：{0}")]
    Image(#[from] image::ImageError),
    #[error("无效参数：{0}")]
    InvalidInput(String),
}

pub type AppResult<T> = Result<T, AppError>;

pub trait CommandResult<T> {
    fn command(self) -> Result<T, String>;
}

impl<T> CommandResult<T> for AppResult<T> {
    fn command(self) -> Result<T, String> {
        self.map_err(|error| error.to_string())
    }
}
