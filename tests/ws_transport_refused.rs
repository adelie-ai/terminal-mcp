#![deny(warnings)]

// MF-12: terminal-mcp must refuse to serve the websocket transport.
//
// terminal-mcp executes arbitrary shell commands; an unauthenticated network
// transport would hand a remote shell to anyone who can reach the port
// (`serve --transport websocket --host 0.0.0.0`). The server is stdio-served
// in practice, so the websocket transport must be rejected outright at
// startup rather than left one CLI flag away.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Spawn `terminal-mcp serve --transport websocket` and require it to exit
/// promptly with a failure instead of binding a listener.
#[test]
fn serve_websocket_is_refused() {
    let exe = env!("CARGO_BIN_EXE_terminal-mcp");

    let mut child = Command::new(exe)
        // --port 0 keeps the test hermetic if a regression ever lets the
        // server get as far as binding a socket.
        .args(["serve", "--transport", "websocket", "--port", "0"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn terminal-mcp serve --transport websocket");

    // The refusal happens before any I/O, so the process should exit almost
    // immediately. Poll with a generous deadline; if it is still running it
    // accepted the transport (the bug), so kill it and fail.
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        match child.try_wait().expect("try_wait on child") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                child.kill().expect("kill lingering websocket server");
                child.wait().expect("reap killed child");
                panic!("terminal-mcp served the websocket transport instead of refusing it");
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };

    assert!(
        !status.success(),
        "serve --transport websocket must exit with a failure status"
    );

    let mut stderr = String::new();
    use std::io::Read;
    child
        .stderr
        .take()
        .expect("child stderr")
        .read_to_string(&mut stderr)
        .expect("read child stderr");
    assert!(
        stderr.to_lowercase().contains("websocket"),
        "refusal error should name the websocket transport; stderr was: {stderr}"
    );
}
