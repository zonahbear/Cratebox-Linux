//! Commands from outside the window: `cratebox --cmd beat1 left` sends one line over a Unix datagram
//! socket to the running app. The Cratebox OS window manager binds Command(Super)-key shortcuts this way,
//! since winit/egui drop the Super modifier on Linux. Listener on its own thread; lines come out of a
//! channel that the UI thread drains each frame.

use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

pub fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d).join("cratebox.sock"),
        _ => crate::store::data_dir().join("cratebox.sock"),
    }
}

/// start listening (replaces a stale socket); `wake` runs after each command so an idle window repaints
pub fn listen(wake: impl Fn() + Send + 'static) -> Option<Receiver<String>> {
    let p = socket_path();
    let _ = std::fs::remove_file(&p);
    let sock = UnixDatagram::bind(&p).map_err(|e| eprintln!("cratebox: no command socket at {}: {e}", p.display())).ok()?;
    let (tx, rx) = channel();
    std::thread::Builder::new().name("remote".into()).spawn(move || {
        let mut buf = [0u8; 256];
        while let Ok(n) = sock.recv(&mut buf) {
            if tx.send(String::from_utf8_lossy(&buf[..n]).trim().to_string()).is_err() { break; }
            wake();
        }
    }).ok()?;
    Some(rx)
}

/// send one command to the running app
pub fn send(cmd: &str) -> std::io::Result<()> {
    UnixDatagram::unbound()?.send_to(cmd.as_bytes(), socket_path()).map(|_| ())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_sent_command_comes_out_of_the_listener() {
        let dir = std::env::temp_dir().join(format!("cratebox-remote-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // the only test in this binary that touches XDG_RUNTIME_DIR
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
        let rx = super::listen(|| {}).unwrap();
        super::send("beat1 left").unwrap();
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap(), "beat1 left");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
