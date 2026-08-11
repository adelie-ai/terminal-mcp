#![deny(warnings)]

// MF-12: terminal-mcp serves the stdio transport only.
//
// terminal-mcp runs arbitrary shell commands. A network or socket listener
// would give a shell to whoever reaches it, so the websocket and unix
// transports are refused at startup rather than left one CLI flag away
// (`serve --transport websocket --host 0.0.0.0`).
//
// The refusal is the only signal a reader gets when a document offers a
// transport this binary does not serve, so these tests pin the exit status and
// the text of the message as well as the refusal itself.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Run `terminal-mcp serve` with `args` and return its exit status and stderr.
///
/// The refusal happens before any I/O, so the process should exit almost
/// immediately. Poll with a generous deadline; if it is still running it
/// accepted the transport (the bug), so kill it and fail.
fn serve_and_capture(args: &[&str]) -> (std::process::ExitStatus, String) {
    let exe = env!("CARGO_BIN_EXE_terminal-mcp");

    let mut child = Command::new(exe)
        .arg("serve")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn terminal-mcp serve");

    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        match child.try_wait().expect("try_wait on child") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                child.kill().expect("kill lingering server");
                child.wait().expect("reap killed child");
                panic!("terminal-mcp served {args:?} instead of refusing it");
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    };

    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("child stderr")
        .read_to_string(&mut stderr)
        .expect("read child stderr");

    (status, stderr)
}

/// `serve --transport websocket` must exit with a failure instead of binding a
/// listener, and the message must name the transport it refused.
#[test]
fn serve_websocket_is_refused() {
    // --port 0 keeps the test hermetic if a regression ever lets the server
    // get as far as binding a socket.
    let (status, stderr) = serve_and_capture(&["--transport", "websocket", "--port", "0"]);

    assert!(
        !status.success(),
        "serve --transport websocket must exit with a failure status"
    );
    assert!(
        stderr.to_lowercase().contains("websocket"),
        "refusal error should name the websocket transport; stderr was: {stderr}"
    );
}

/// `serve --transport unix` must be refused too. terminal-mcp never opts into
/// mcp-core's unix transport, so stdio is the whole supported set, and the
/// documents say so.
#[test]
fn serve_unix_is_refused() {
    let (status, stderr) =
        serve_and_capture(&["--transport", "unix", "--socket-path", "/dev/null"]);

    assert!(
        !status.success(),
        "serve --transport unix must exit with a failure status"
    );
    assert!(
        stderr.to_lowercase().contains("unix"),
        "refusal error should name the unix transport; stderr was: {stderr}"
    );
}

/// The refusal must tell a reader what to do next.
///
/// A reader who followed an out-of-date document cannot otherwise tell a wrong
/// flag from a wrong document. Two properties: the message renders the error's
/// `Display` text rather than the `Debug` dump Rust prints by default from
/// `main() -> Result` (which shows the enum variant and hides the wording), and
/// it names the transport this server does serve.
#[test]
fn transport_refusal_message_names_the_supported_transport() {
    let (_status, stderr) = serve_and_capture(&["--transport", "websocket", "--port", "0"]);

    assert!(
        !stderr.contains("Config("),
        "refusal should print the error's Display text, not the Debug dump; stderr was: {stderr}"
    );
    assert!(
        stderr.contains("--transport stdio"),
        "refusal should point at the transport this server serves; stderr was: {stderr}"
    );
}
