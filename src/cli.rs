use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};

use clap::Parser;

use crate::error::AppError;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "kirara",
    version,
    about = "PT 自动化管理控制台",
    long_about = "启动 kirara Web 服务。\n\n默认行为:\n- 监听地址: 127.0.0.1:3000\n- 数据库: ./data/kirara.db\n\n指定 --data-dir 后，数据库和运行数据都写入该目录。\n\n示例:\n- kirara\n- kirara -H 127.0.0.1 -p 8080\n- kirara -d ./runtime-data\n- KIRARA_DATA_DIR=/data kirara"
)]
pub struct Cli {
    #[arg(
        short = 'H',
        long,
        env = "KIRARA_HOST",
        default_value = "127.0.0.1",
        help = "Web 服务监听地址 (env: KIRARA_HOST)"
    )]
    pub host: String,

    #[arg(
        short = 'p',
        long,
        env = "KIRARA_PORT",
        default_value_t = 3000,
        help = "Web 服务监听端口 (env: KIRARA_PORT)"
    )]
    pub port: u16,

    #[arg(
        short = 'd',
        long = "data-dir",
        env = "KIRARA_DATA_DIR",
        value_name = "DIR",
        help = "应用数据目录。指定后数据库和运行数据都写入该目录 (env: KIRARA_DATA_DIR)"
    )]
    pub data_dir: Option<PathBuf>,
}

impl Cli {
    pub fn resolve_paths(&self, current_dir: &Path) -> (PathBuf, PathBuf) {
        if let Some(data_dir) = &self.data_dir {
            (data_dir.clone(), data_dir.clone())
        } else {
            (current_dir.to_path_buf(), current_dir.join("data"))
        }
    }

    pub fn resolve_listen_addr(&self) -> Result<SocketAddr, AppError> {
        let mut addrs = (self.host.as_str(), self.port)
            .to_socket_addrs()
            .map_err(|error| AppError::InvalidConfig {
                message: format!(
                    "invalid listen address {}:{}: {}",
                    self.host, self.port, error
                ),
            })?;

        addrs.next().ok_or_else(|| AppError::InvalidConfig {
            message: format!("no socket address resolved for {}:{}", self.host, self.port),
        })
    }
}
