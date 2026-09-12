//! Establish a browser transport through the configured system proxy.
use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub(super) async fn connect(
    host: &str,
    port: u16,
    proxy: Option<&str>,
) -> Result<TcpStream, String> {
    let Some(proxy) = proxy.map(str::trim).filter(|p| !p.is_empty()) else {
        return TcpStream::connect((host, port))
            .await
            .map_err(|e| e.to_string());
    };
    let url = reqwest::Url::parse(proxy).map_err(|_| "系统代理地址无效")?;
    if !matches!(url.scheme(), "http" | "socks5" | "socks5h") {
        return Err("Lightpanda 系统代理目前支持 http://、socks5:// 或 socks5h://".into());
    }
    let proxy_host = url.host_str().ok_or("系统代理缺少主机名")?;
    let proxy_port = url
        .port()
        .unwrap_or(if url.scheme() == "http" { 80 } else { 1080 });
    let mut stream = TcpStream::connect((proxy_host, proxy_port))
        .await
        .map_err(|e| format!("连接系统代理失败：{e}"))?;
    let username = urlencoding::decode(url.username()).map_err(|_| "代理用户名编码无效")?;
    let password =
        urlencoding::decode(url.password().unwrap_or_default()).map_err(|_| "代理密码编码无效")?;
    if url.scheme() == "http" {
        let authority = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        let auth = if username.is_empty() && password.is_empty() {
            String::new()
        } else {
            format!(
                "Proxy-Authorization: Basic {}\r\n",
                base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
            )
        };
        stream
            .write_all(
                format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n{auth}\r\n")
                    .as_bytes(),
            )
            .await
            .map_err(|_| "发送代理 CONNECT 失败")?;
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            if headers.len() >= 16384 {
                return Err("代理 CONNECT 响应头过大".into());
            }
            headers.push(
                stream
                    .read_u8()
                    .await
                    .map_err(|_| "读取代理 CONNECT 响应失败")?,
            );
        }
        let status = String::from_utf8_lossy(&headers)
            .split_whitespace()
            .nth(1)
            .unwrap_or_default()
            .parse::<u16>()
            .unwrap_or(0);
        if status != 200 {
            return Err(format!(
                "系统代理 CONNECT 被拒绝（HTTP {status}），请检查代理连接和认证"
            ));
        }
    } else {
        let auth = !username.is_empty() || !password.is_empty();
        stream
            .write_all(if auth { &[5, 2, 0, 2] } else { &[5, 1, 0] })
            .await
            .map_err(|_| "发送 SOCKS5 握手失败")?;
        let mut reply = [0; 2];
        stream
            .read_exact(&mut reply)
            .await
            .map_err(|_| "读取 SOCKS5 握手失败")?;
        if reply[0] != 5 {
            return Err("SOCKS5 代理响应版本无效".into());
        }
        match reply[1] {
            0 => {}
            2 if auth && username.len() <= 255 && password.len() <= 255 => {
                let mut credentials = vec![1, username.len() as u8];
                credentials.extend_from_slice(username.as_bytes());
                credentials.push(password.len() as u8);
                credentials.extend_from_slice(password.as_bytes());
                stream
                    .write_all(&credentials)
                    .await
                    .map_err(|_| "发送 SOCKS5 认证失败")?;
                stream
                    .read_exact(&mut reply)
                    .await
                    .map_err(|_| "读取 SOCKS5 认证失败")?;
                if reply != [1, 0] {
                    return Err("SOCKS5 代理认证失败".into());
                }
            }
            _ => return Err("SOCKS5 代理不支持当前认证方式".into()),
        }
        let mut request = vec![5, 1, 0];
        match host.parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V4(ip)) => {
                request.push(1);
                request.extend_from_slice(&ip.octets());
            }
            Ok(std::net::IpAddr::V6(ip)) => {
                request.push(4);
                request.extend_from_slice(&ip.octets());
            }
            Err(_) if host.len() <= 255 => {
                request.extend_from_slice(&[3, host.len() as u8]);
                request.extend_from_slice(host.as_bytes());
            }
            _ => return Err("浏览器主机名过长".into()),
        }
        request.extend_from_slice(&port.to_be_bytes());
        stream
            .write_all(&request)
            .await
            .map_err(|_| "发送 SOCKS5 连接请求失败")?;
        let mut header = [0; 4];
        stream
            .read_exact(&mut header)
            .await
            .map_err(|_| "读取 SOCKS5 连接结果失败")?;
        if header[0] != 5 || header[1] != 0 {
            return Err(format!("SOCKS5 代理连接被拒绝（状态 {}）", header[1]));
        }
        let len = match header[3] {
            1 => 4,
            4 => 16,
            3 => stream.read_u8().await.map_err(|_| "读取 SOCKS5 地址失败")? as usize,
            _ => return Err("SOCKS5 地址类型无效".into()),
        };
        stream
            .read_exact(&mut vec![0; len + 2])
            .await
            .map_err(|_| "读取 SOCKS5 地址失败")?;
    }
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn http_proxy_auth_tunnel_preserves_first_browser_bytes() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("http://user:p%40ss@{}", listener.local_addr().unwrap());
        let peer = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                header.push(stream.read_u8().await.unwrap());
            }
            let request = String::from_utf8(header).unwrap();
            assert!(request.starts_with("CONNECT browser.example:443 HTTP/1.1\r\n"));
            assert!(request.contains("Proxy-Authorization: Basic dXNlcjpwQHNz\r\n"));
            stream
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\nBROWSER")
                .await
                .unwrap();
        });
        let mut connection = connect("browser.example", 443, Some(&proxy)).await.unwrap();
        let mut bytes = [0; 7];
        connection.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"BROWSER");
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn http_proxy_rejection_reports_status_without_credentials() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("http://user:secret@{}", listener.local_addr().unwrap());
        let peer = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                header.push(stream.read_u8().await.unwrap());
            }
            stream
                .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n")
                .await
                .unwrap();
        });
        let error = connect("browser.example", 443, Some(&proxy))
            .await
            .unwrap_err();
        assert!(error.contains("407"));
        assert!(!error.contains("secret"));
        peer.await.unwrap();
    }

    #[tokio::test]
    async fn socks_proxy_resolves_browser_hostname_remotely() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("socks5h://{}", listener.local_addr().unwrap());
        let peer = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut greeting = [0; 3];
            stream.read_exact(&mut greeting).await.unwrap();
            assert_eq!(greeting, [5, 1, 0]);
            stream.write_all(&[5, 0]).await.unwrap();
            let mut request = [0; 5];
            stream.read_exact(&mut request).await.unwrap();
            assert_eq!(&request[..4], &[5, 1, 0, 3]);
            let mut host = vec![0; request[4] as usize];
            stream.read_exact(&mut host).await.unwrap();
            assert_eq!(host, b"browser.example");
            assert_eq!(stream.read_u16().await.unwrap(), 443);
            stream
                .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])
                .await
                .unwrap();
        });
        connect("browser.example", 443, Some(&proxy)).await.unwrap();
        peer.await.unwrap();
    }
}
