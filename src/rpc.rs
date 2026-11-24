//! herdr's socket API: one JSON request per line, one JSON response per line.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

/// Anything that answers herdr API requests. The real one is [`Client`]; tests
/// substitute a fake.
pub trait Herdr {
    /// Sends `method` with `params` and returns the response's `result`.
    fn call(&mut self, method: &str, params: Value) -> Result<Value>;
}

pub struct Client {
    socket: PathBuf,
}

impl Client {
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            socket: socket.into(),
        }
    }

    /// The session this process runs in, from `HERDR_SOCKET_PATH`.
    pub fn from_env() -> Result<Self> {
        let socket = std::env::var_os("HERDR_SOCKET_PATH")
            .context("HERDR_SOCKET_PATH is not set; run herdr-oil from inside herdr")?;
        Ok(Self::new(socket))
    }
}

impl Herdr for Client {
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let mut stream = UnixStream::connect(&self.socket)
            .with_context(|| format!("connecting to {}", self.socket.display()))?;
        let mut request = json!({ "id": "herdr-oil", "method": method, "params": params })
            .to_string()
            .into_bytes();
        request.push(b'\n');
        stream.write_all(&request)?;

        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        if line.is_empty() {
            bail!("{method}: herdr closed the connection without answering");
        }
        let mut response: Value =
            serde_json::from_str(&line).with_context(|| format!("{method}: bad response"))?;
        if let Some(error) = response.get("error") {
            let message = error["message"].as_str().unwrap_or("unknown error");
            return Err(anyhow!("{method}: {message}"));
        }
        Ok(response["result"].take())
    }
}
