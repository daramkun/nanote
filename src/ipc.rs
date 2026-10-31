//! Single-instance support: a later `nanote file...` hands its files to the running instance
//! over a loopback TCP socket. With `wait`, the connection stays open until the tab is closed
//! (used when nanote is a Git editor).

use std::{
    sync::Arc,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    sync::mpsc::Sender,
    time::Duration,
};

use serde::{Deserialize, Serialize};

pub const ADDR: &str = "127.0.0.1:47831";
const MAGIC: &str = "nanote-open-v1";

#[derive(Serialize, Deserialize)]
struct Message {
    magic: String,
    paths: Vec<String>,
    wait: bool,
}

pub struct Incoming {
    pub paths: Vec<PathBuf>,
    pub wait: bool,
    /// Kept open while `wait` is set; the app writes `DONE\n` when the file is closed.
    pub stream: TcpStream,
}

/// Try to hand `paths` to an already running instance. Returns true if one accepted them
/// (and, with `wait`, the file has since been closed).
pub fn forward(paths: &[PathBuf], wait: bool) -> bool {
    let addr: SocketAddr = ADDR.parse().unwrap();
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(300)) else {
        return false;
    };
    let msg = Message {
        magic: MAGIC.into(),
        paths: paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        wait,
    };
    let Ok(line) = serde_json::to_string(&msg) else {
        return false;
    };
    if writeln!(stream, "{line}").is_err() {
        return false;
    }
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let mut reader = BufReader::new(&stream);
    let mut ack = String::new();
    if reader.read_line(&mut ack).is_err() || ack.trim() != "OK" {
        return false;
    }
    if wait {
        let _ = stream.set_read_timeout(None);
        let mut sink = [0u8; 64];
        // Blocks until the server writes DONE or drops the connection.
        while matches!(reader.read(&mut sink), Ok(n) if n > 0) {}
    }
    true
}

/// `notify` is called (from a background thread) after a request was queued so the UI can wake up.
pub fn serve(listener: TcpListener, tx: Sender<Incoming>, notify: Arc<dyn Fn() + Send + Sync>) {
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let (tx, notify) = (tx.clone(), notify.clone());
            std::thread::spawn(move || handle(conn, tx, notify));
        }
    });
}

fn handle(mut conn: TcpStream, tx: Sender<Incoming>, notify: Arc<dyn Fn() + Send + Sync>) {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(2)));
    let mut line = String::new();
    let Ok(clone) = conn.try_clone() else { return };
    if BufReader::new(clone.take(1 << 20))
        .read_line(&mut line)
        .is_err()
    {
        return;
    }
    let Ok(msg) = serde_json::from_str::<Message>(&line) else {
        return;
    };
    if msg.magic != MAGIC {
        return;
    }
    let _ = conn.set_read_timeout(None);
    if writeln!(conn, "OK").is_err() {
        return;
    }
    let paths = msg.paths.into_iter().map(PathBuf::from).collect();
    if tx
        .send(Incoming {
            paths,
            wait: msg.wait,
            stream: conn,
        })
        .is_ok()
    {
        notify();
    }
}
