use std::net::{SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};

use clap::Parser;

use crate::ListenEndpoint;
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

    #[arg(long, env = "KIRARA_SOCKET", hide = true)]
    pub socket: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn numeric_port_and_data_directory_remain_compatible() {
        let cli = Cli::try_parse_from(["kirara", "-H", "127.0.0.1", "-p", "8080", "-d", "runtime"])
            .unwrap();
        assert_eq!(
            cli.resolve_endpoint().unwrap(),
            ListenEndpoint::Tcp("127.0.0.1:8080".parse().unwrap())
        );
        assert_eq!(
            cli.resolve_paths(Path::new("/unused")),
            (PathBuf::from("runtime"), PathBuf::from("runtime"))
        );
        assert!(Cli::try_parse_from(["kirara", "-p", "65536"]).is_err());
    }

    #[test]
    fn local_transport_is_omitted_from_public_help() {
        let help = Cli::command().render_long_help().to_string();
        assert!(!help.contains("--socket"));
        assert!(!help.contains("KIRARA_SOCKET"));
        assert!(help.contains("--port"));
    }

    #[cfg(unix)]
    #[test]
    fn local_transport_does_not_resolve_a_tcp_host() {
        let cli = Cli::try_parse_from([
            "kirara",
            "--socket",
            "/tmp/kirara-test.sock",
            "-H",
            "invalid host",
        ])
        .unwrap();
        assert_eq!(
            cli.resolve_endpoint().unwrap(),
            ListenEndpoint::Unix("/tmp/kirara-test.sock".into())
        );
    }
}

impl Cli {
    pub fn resolve_endpoint(&self) -> Result<ListenEndpoint, AppError> {
        if let Some(socket) = &self.socket {
            if socket.is_empty() {
                return Err(AppError::InvalidConfig {
                    message: "local listener name must not be empty".into(),
                });
            }
            #[cfg(unix)]
            return Ok(ListenEndpoint::Unix(PathBuf::from(socket)));
            #[cfg(windows)]
            return Ok(ListenEndpoint::NamedPipe(socket.clone()));
        }
        self.resolve_listen_addr().map(ListenEndpoint::Tcp)
    }

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
