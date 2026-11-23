//! A Blossom media server for the photo tests (#94). It takes uploads (BUD-02 `PUT /upload`)
//! and serves them back, from memory. MDK wants the upload's descriptor to give an address on
//! this server that names the blob's hash.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

type Blobs = Arc<Mutex<HashMap<String, Vec<u8>>>>;

pub struct TestBlossom {
    url: String,
    blobs: Blobs,
    task: JoinHandle<()>,
}

impl TestBlossom {
    pub async fn start() -> Self {
        Self::start_at("127.0.0.1:0").await
    }

    /// A server at `address`, such as `127.0.0.1:7778`.
    pub async fn start_at(address: &str) -> Self {
        let listener = TcpListener::bind(address).await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let blobs = Blobs::default();
        let task = tokio::spawn(serve(listener, url.clone(), blobs.clone()));
        Self { url, blobs, task }
    }

    pub fn url(&self) -> String {
        self.url.clone()
    }

    /// Every blob uploaded so far.
    pub fn blobs(&self) -> Vec<Vec<u8>> {
        self.blobs.lock().unwrap().values().cloned().collect()
    }
}

impl Drop for TestBlossom {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(listener: TcpListener, url: String, blobs: Blobs) {
    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(answer(stream, url.clone(), blobs.clone()));
    }
}

/// Answers one request, then closes the connection.
async fn answer(stream: TcpStream, url: String, blobs: Blobs) {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).await.unwrap_or(0) == 0 {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let path = parts.next().unwrap_or_default().to_owned();
    let mut length = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
            return;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    if reader.read_exact(&mut body).await.is_err() {
        return;
    }

    let not_found = || ("404 Not Found", "text/plain", b"not found".to_vec());
    let (status, content_type, response) = match (method.as_str(), path.as_str()) {
        ("PUT", "/upload") => {
            let hash = hex::encode(Sha256::digest(&body));
            let size = body.len();
            blobs.lock().unwrap().insert(hash.clone(), body);
            let descriptor = format!(
                r#"{{"url":"{url}/{hash}.jpg","sha256":"{hash}","size":{size},"type":"image/jpeg","uploaded":0}}"#
            );
            ("200 OK", "application/json", descriptor.into_bytes())
        }
        ("GET", path) => {
            let hash = path.trim_start_matches('/').split('.').next();
            match hash.and_then(|hash| blobs.lock().unwrap().get(hash).cloned()) {
                Some(blob) => ("200 OK", "image/jpeg", blob),
                None => not_found(),
            }
        }
        _ => not_found(),
    };
    let mut stream = reader.into_inner();
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.len()
    );
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(&response).await;
    let _ = stream.shutdown().await;
}
