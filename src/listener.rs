use std::{
    fmt, io,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

#[cfg(unix)]
use std::{
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::PathBuf,
};
#[cfg(windows)]
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ListenEndpoint {
    Tcp(SocketAddr),
    #[cfg(unix)]
    Unix(PathBuf),
    #[cfg(windows)]
    NamedPipe(String),
}

impl fmt::Display for ListenEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tcp(address) => address.fmt(f),
            #[cfg(unix)]
            Self::Unix(path) => path.display().fmt(f),
            #[cfg(windows)]
            Self::NamedPipe(name) => name.fmt(f),
        }
    }
}

impl ListenEndpoint {
    pub async fn bind(&self) -> io::Result<Listener> {
        let inner = match self {
            Self::Tcp(address) => ListenerInner::Tcp(TcpListener::bind(address).await?),
            #[cfg(unix)]
            Self::Unix(path) => {
                // Keep cleanup independent of later changes to the process directory.
                let absolute = std::path::absolute(path)?;
                let parent = absolute.parent().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "socket path has no parent")
                })?;
                let name = absolute.file_name().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "socket path has no file name")
                })?;
                let path = parent.canonicalize()?.join(name);

                // bind itself provides the no-clobber check, including stale sockets,
                // regular files, and symlinks. Never unlink an existing path to bind.
                let listener = UnixListener::bind(&path)?;
                let metadata = std::fs::symlink_metadata(&path)?;
                let socket = SocketPath {
                    path,
                    device: metadata.dev(),
                    inode: metadata.ino(),
                };
                // Construct the guard before fallible setup so errors also clean up.
                std::fs::set_permissions(&socket.path, std::fs::Permissions::from_mode(0o600))?;
                ListenerInner::Unix { listener, socket }
            }
            #[cfg(windows)]
            Self::NamedPipe(name) => {
                validate_pipe_name(name)?;
                let server = pipe_options(true).create(name)?;
                ListenerInner::NamedPipe {
                    server,
                    name: name.clone(),
                }
            }
        };
        Ok(Listener { inner })
    }

    pub async fn connect(&self) -> io::Result<Stream> {
        match self {
            Self::Tcp(address) => TcpStream::connect(address).await.map(Stream::Tcp),
            #[cfg(unix)]
            Self::Unix(path) => UnixStream::connect(path).await.map(Stream::Unix),
            #[cfg(windows)]
            Self::NamedPipe(name) => {
                validate_pipe_name(name)?;
                // A busy pipe is expected while the server replaces an instance.
                // Bound the retry so an occupied but unresponsive server cannot
                // keep a desktop protocol request pending indefinitely.
                tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        match ClientOptions::new().open(name) {
                            Ok(client) => return Ok(Stream::NamedPipeClient(client)),
                            Err(error) if error.raw_os_error() == Some(231) => {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                            Err(error) => return Err(error),
                        }
                    }
                })
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "named pipe remained busy"))?
            }
        }
    }
}

#[derive(Debug)]
pub struct Listener {
    inner: ListenerInner,
}

#[derive(Debug)]
enum ListenerInner {
    Tcp(TcpListener),
    #[cfg(unix)]
    Unix {
        listener: UnixListener,
        socket: SocketPath,
    },
    #[cfg(windows)]
    NamedPipe {
        server: NamedPipeServer,
        name: String,
    },
}

impl Listener {
    pub fn endpoint(&self) -> io::Result<ListenEndpoint> {
        match &self.inner {
            ListenerInner::Tcp(listener) => listener.local_addr().map(ListenEndpoint::Tcp),
            #[cfg(unix)]
            ListenerInner::Unix { socket, .. } => Ok(ListenEndpoint::Unix(socket.path.clone())),
            #[cfg(windows)]
            ListenerInner::NamedPipe { name, .. } => Ok(ListenEndpoint::NamedPipe(name.clone())),
        }
    }

    async fn accept_connection(&mut self) -> io::Result<(Stream, String)> {
        match &mut self.inner {
            ListenerInner::Tcp(listener) => listener
                .accept()
                .await
                .map(|(stream, address)| (Stream::Tcp(stream), address.to_string())),
            #[cfg(unix)]
            ListenerInner::Unix { listener, .. } => {
                listener.accept().await.map(|(stream, address)| {
                    let address = address
                        .as_pathname()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "local".to_owned());
                    (Stream::Unix(stream), address)
                })
            }
            #[cfg(windows)]
            ListenerInner::NamedPipe { server, name } => {
                if let Err(error) = server.connect().await {
                    // In particular, recover if a client disconnects before it
                    // is accepted (ERROR_NO_DATA).
                    let _ = server.disconnect();
                    return Err(error);
                }
                // Always retain an instance while handing the connected one to
                // axum. Even a failed replacement must not release this name.
                loop {
                    match pipe_options(false).create(name.as_str()) {
                        Ok(next) => {
                            let connected = std::mem::replace(server, next);
                            return Ok((Stream::NamedPipeServer(connected), name.clone()));
                        }
                        Err(error) => retry_accept(error).await,
                    }
                }
            }
        }
    }
}

impl axum::serve::Listener for Listener {
    type Io = Stream;
    type Addr = String;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.accept_connection().await {
                Ok(connection) => return connection,
                Err(error) => retry_accept(error).await,
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.endpoint().map(|endpoint| endpoint.to_string())
    }
}

async fn retry_accept(error: io::Error) {
    tracing::warn!(%error, "listener could not accept a connection; retrying");
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[cfg(unix)]
#[derive(Debug)]
struct SocketPath {
    path: PathBuf,
    device: u64,
    inode: u64,
}

#[cfg(unix)]
impl Drop for SocketPath {
    fn drop(&mut self) {
        // Do not remove a file or another listener that replaced our path.
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path)
            && metadata.file_type().is_socket()
            && metadata.dev() == self.device
            && metadata.ino() == self.inode
            && let Err(error) = std::fs::remove_file(&self.path)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(path = %self.path.display(), %error, "could not remove socket");
        }
    }
}

#[cfg(windows)]
fn validate_pipe_name(name: &str) -> io::Result<()> {
    const PREFIX: &str = r"\\.\pipe\";
    if !name.to_ascii_lowercase().starts_with(PREFIX)
        || name.len() == PREFIX.len()
        || name.contains('\0')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "named pipe must have a local \\\\.\\pipe\\ name",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn pipe_options(first: bool) -> ServerOptions {
    let mut options = ServerOptions::new();
    options
        .first_pipe_instance(first)
        .reject_remote_clients(true);
    options
}

#[derive(Debug)]
pub enum Stream {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(UnixStream),
    #[cfg(windows)]
    NamedPipeServer(NamedPipeServer),
    #[cfg(windows)]
    NamedPipeClient(NamedPipeClient),
}

// All transport types are Unpin, so pin projection needs no unsafe code.
macro_rules! dispatch_stream {
    ($self:expr, $stream:ident => $operation:expr) => {
        match $self {
            Stream::Tcp($stream) => $operation,
            #[cfg(unix)]
            Stream::Unix($stream) => $operation,
            #[cfg(windows)]
            Stream::NamedPipeServer($stream) => $operation,
            #[cfg(windows)]
            Stream::NamedPipeClient($stream) => $operation,
        }
    };
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        dispatch_stream!(self.get_mut(), stream => Pin::new(stream).poll_read(cx, buffer))
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        dispatch_stream!(self.get_mut(), stream => Pin::new(stream).poll_write(cx, buffer))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        dispatch_stream!(self.get_mut(), stream => Pin::new(stream).poll_flush(cx))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        dispatch_stream!(self.get_mut(), stream => Pin::new(stream).poll_shutdown(cx))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffers: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        dispatch_stream!(self.get_mut(), stream => Pin::new(stream).poll_write_vectored(cx, buffers))
    }

    fn is_write_vectored(&self) -> bool {
        dispatch_stream!(self, stream => stream.is_write_vectored())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::serve::Listener as _;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn round_trip(listener: &mut Listener) {
        let endpoint = listener.endpoint().unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            let server = async {
                let (mut stream, _) = listener.accept().await;
                let mut request = [0; 4];
                stream.read_exact(&mut request).await.unwrap();
                assert_eq!(&request, b"ping");
                stream.write_all(b"pong").await.unwrap();
            };
            let client = async {
                let mut stream = endpoint.connect().await.unwrap();
                stream.write_all(b"ping").await.unwrap();
                let mut response = [0; 4];
                stream.read_exact(&mut response).await.unwrap();
                assert_eq!(&response, b"pong");
            };
            tokio::join!(server, client);
        })
        .await
        .expect("transport round trip timed out");
    }

    #[tokio::test]
    async fn tcp_round_trip_uses_assigned_port() {
        let mut listener = ListenEndpoint::Tcp("127.0.0.1:0".parse().unwrap())
            .bind()
            .await
            .unwrap();
        let ListenEndpoint::Tcp(address) = listener.endpoint().unwrap() else {
            panic!("expected TCP endpoint");
        };
        assert_ne!(address.port(), 0);
        round_trip(&mut listener).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_round_trip_permissions_and_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("listener.sock");
        let endpoint = ListenEndpoint::Unix(path.clone());
        let mut listener = endpoint.bind().await.unwrap();
        assert_eq!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        round_trip(&mut listener).await;
        drop(listener);
        assert!(!path.exists());
        let listener = endpoint.bind().await.unwrap();
        drop(listener);
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_bind_preserves_live_socket_and_regular_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("listener.sock");
        let endpoint = ListenEndpoint::Unix(path.clone());
        let mut first = endpoint.bind().await.unwrap();
        let error = endpoint.bind().await.unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
        round_trip(&mut first).await;
        drop(first);

        std::fs::write(&path, "preserve this file").unwrap();
        assert!(endpoint.bind().await.is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "preserve this file"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_cleanup_preserves_replacement_path() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("listener.sock");
        let endpoint = ListenEndpoint::Unix(path.clone());
        let first = endpoint.bind().await.unwrap();
        std::fs::remove_file(&path).unwrap();
        let mut replacement = endpoint.bind().await.unwrap();
        drop(first);
        assert!(path.exists());
        round_trip(&mut replacement).await;
        drop(replacement);
        assert!(!path.exists());

        let listener = endpoint.bind().await.unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, "replacement").unwrap();
        drop(listener);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "replacement");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_bind_preserves_stale_socket_and_symlink() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("listener.sock");
        let endpoint = ListenEndpoint::Unix(path.clone());
        drop(UnixListener::bind(&path).unwrap());
        assert!(endpoint.bind().await.is_err());
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_socket()
        );
        std::fs::remove_file(&path).unwrap();

        let target = directory.path().join("target");
        std::fs::write(&target, "keep").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(endpoint.bind().await.is_err());
        assert!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(target).unwrap(), "keep");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn named_pipe_round_trip_conflict_continuity_and_release() {
        let directory = tempfile::tempdir().unwrap();
        let unique = directory.path().file_name().unwrap().to_string_lossy();
        let endpoint = ListenEndpoint::NamedPipe(format!(
            r"\\.\pipe\kirara-test-{}-{unique}",
            std::process::id()
        ));
        let mut listener = endpoint.bind().await.unwrap();
        assert!(endpoint.bind().await.is_err());
        for _ in 0..4 {
            round_trip(&mut listener).await;
        }
        // The waiting instance keeps the pipe present after accepted clients exit.
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut client = endpoint.connect().await.unwrap();
            let (mut server, _) = listener.accept().await;
            client.write_all(b"x").await.unwrap();
            assert_eq!(server.read_u8().await.unwrap(), b'x');
        })
        .await
        .unwrap();
        drop(listener);
        assert!(endpoint.connect().await.is_err());
        let mut listener = endpoint.bind().await.unwrap();
        round_trip(&mut listener).await;
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn named_pipe_recovers_from_client_closing_before_accept() {
        let directory = tempfile::tempdir().unwrap();
        let unique = directory.path().file_name().unwrap().to_string_lossy();
        let endpoint = ListenEndpoint::NamedPipe(format!(
            r"\\.\pipe\kirara-disconnect-test-{}-{unique}",
            std::process::id()
        ));
        let mut listener = endpoint.bind().await.unwrap();
        drop(endpoint.connect().await.unwrap());
        tokio::time::timeout(Duration::from_secs(5), async {
            let server = async {
                loop {
                    let (mut stream, _) = listener.accept().await;
                    // Depending on timing, Windows can reject the abandoned
                    // connection during accept or hand us a stream at EOF.
                    if let Ok(byte) = stream.read_u8().await {
                        assert_eq!(byte, b'x');
                        stream.write_all(b"y").await.unwrap();
                        break;
                    }
                }
            };
            let client = async {
                let mut stream = endpoint.connect().await.unwrap();
                stream.write_all(b"x").await.unwrap();
                assert_eq!(stream.read_u8().await.unwrap(), b'y');
            };
            tokio::join!(server, client);
        })
        .await
        .expect("named pipe did not recover after a disconnected client");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn named_pipe_rejects_nonlocal_names() {
        let endpoint = ListenEndpoint::NamedPipe(r"\\remote\pipe\kirara-test".to_owned());
        assert_eq!(
            endpoint.bind().await.unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            endpoint.connect().await.unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
}
