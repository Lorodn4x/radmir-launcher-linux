use crate::error::AppResult;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

/// Guest/in-game-login contract: a loopback-only /token endpoint returning
/// an empty token. No account credentials are imported from the old launcher.
pub struct TokenServer {
    pub port: u16,
    task: JoinHandle<()>,
}
impl TokenServer {
    pub async fn start() -> AppResult<Self> {
        Self::start_on(0).await
    }
    pub async fn start_on(port: u16) -> AppResult<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port)).await?;
        let port = listener.local_addr()?.port();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                // Bound one request's time and bytes; the game needs only a tiny GET.
                let _ = tokio::time::timeout(Duration::from_secs(2), async {
                    let mut buffer = [0u8; 4096];
                    let mut used = 0;
                    while used < buffer.len() {
                        let n = stream.read(&mut buffer[used..]).await?;
                        if n == 0 { break; }
                        used += n;
                        if buffer[..used].windows(4).any(|w| w == b"\r\n\r\n") { break; }
                    }
                    let first = buffer[..used].split(|b| *b == b'\r').next().unwrap_or_default();
                    let response = if first == b"GET /token HTTP/1.1" || first == b"GET /token HTTP/1.0" {
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 0\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n"
                    } else {
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    };
                    stream.write_all(response.as_bytes()).await
                }).await;
            }
        });
        Ok(Self { port, task })
    }
}
impl Drop for TokenServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn serves_empty_guest_token_and_rejects_other_routes() {
        let server = TokenServer::start().await.unwrap();
        let client = reqwest::Client::new();
        let url = format!("http://127.0.0.1:{}", server.port);
        let response = client.get(format!("{url}/token")).send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await.unwrap(), "");
        assert_eq!(
            client
                .post(format!("{url}/token"))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
        assert_eq!(
            client
                .get(format!("{url}/other"))
                .send()
                .await
                .unwrap()
                .status(),
            404
        );
    }
}
