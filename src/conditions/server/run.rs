//! Running a server until its log says `Done`, then stopping it.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use crate::conditions::Status;

/// How many log lines a failure shows.
const TAIL: usize = 20;
/// How long a server gets to stop after `stop` before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(60);

/// Starts `command`, waits for the `Done` line of the log, then sends `stop`.
/// Fails when the process exits first or `timeout` passes, with the last
/// lines of the log.
pub fn until_done(mut command: Command, timeout: Duration) -> Status {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return Status::Failed(format!("cannot start the server: {e}")),
    };

    let (tx, rx) = mpsc::channel();
    let stdout = child
        .stdout
        .take()
        .map(|s| Box::new(s) as Box<dyn Read + Send>);
    let stderr = child
        .stderr
        .take()
        .map(|s| Box::new(s) as Box<dyn Read + Send>);
    for stream in [stdout, stderr].into_iter().flatten() {
        let tx = tx.clone();
        thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
    }
    drop(tx);

    let deadline = Instant::now() + timeout;
    let mut tail = VecDeque::with_capacity(TAIL);
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(line) => {
                tracing::debug!(target: "server", "{line}");
                let done = is_done(&line);
                if tail.len() == TAIL {
                    tail.pop_front();
                }
                tail.push_back(line);
                if done {
                    stop(&mut child);
                    return Status::Passed;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                let _ = child.wait();
                return Status::Failed(format!(
                    "no `Done` in the log within {}s; last lines of the log:\n{}",
                    timeout.as_secs(),
                    lines(&tail)
                ));
            }
            Err(RecvTimeoutError::Disconnected) => {
                let status = child
                    .wait()
                    .map_or_else(|e| e.to_string(), |s| s.to_string());
                return Status::Failed(format!(
                    "the server stopped before `Done` ({status}); last lines of the log:\n{}",
                    lines(&tail)
                ));
            }
        }
    }
}

/// `[Server thread/INFO]: Done (5.123s)! For help, type "help"`
fn is_done(line: &str) -> bool {
    line.contains("]: Done (")
}

/// Sends `stop` and waits for the process to exit, killing it after a grace
/// period. The condition already passed: a slow stop is only logged.
fn stop(child: &mut Child) {
    if let Some(stdin) = child.stdin.as_mut() {
        let _ = stdin.write_all(b"stop\n").and_then(|()| stdin.flush());
    }
    let deadline = Instant::now() + STOP_GRACE;
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    tracing::warn!(
        "the server did not stop within {}s of `stop`, killed",
        STOP_GRACE.as_secs()
    );
    let _ = child.kill();
    let _ = child.wait();
}

fn lines(tail: &VecDeque<String>) -> String {
    tail.iter().cloned().collect::<Vec<_>>().join("\n")
}

/// The last lines of a text, for an installer that failed.
pub fn last_lines(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(TAIL)..].join("\n")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn fake(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        command
    }

    const DONE: &str = r#"echo '[Server thread/INFO]: Done (0.42s)! For help, type "help"'"#;

    #[test]
    fn done_line_passes_and_the_server_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let got = dir.path().join("got");
        let script = format!(
            "echo starting; {DONE}; read cmd; echo \"$cmd\" > '{}'",
            got.display()
        );
        let status = until_done(fake(&script), Duration::from_secs(20));
        assert_eq!(status, Status::Passed);
        assert_eq!(std::fs::read_to_string(got).unwrap(), "stop\n");
    }

    #[test]
    fn crash_fails_with_the_last_lines_of_the_log() {
        // The pause keeps stderr after stdout in the merged log.
        let script = "for i in $(seq 1 30); do echo line $i; done; sleep 1; \
                      echo 'Exception in thread main' >&2; exit 3";
        let reason = match until_done(fake(script), Duration::from_secs(20)) {
            Status::Failed(reason) => reason,
            other => panic!("{other:?}"),
        };
        assert!(
            reason.starts_with("the server stopped before `Done`"),
            "{reason}"
        );
        assert!(reason.contains("3"), "exit status shown: {reason}");
        assert!(reason.contains("Exception in thread main"), "{reason}");
        assert!(reason.contains("line 30"), "{reason}");
        assert!(!reason.contains("line 5\n"), "only the tail: {reason}");
    }

    #[test]
    fn timeout_fails_and_kills_the_server() {
        let started = Instant::now();
        let status = until_done(fake("echo loading; exec sleep 30"), Duration::from_secs(1));
        assert!(started.elapsed() < Duration::from_secs(10));
        let reason = status.reason().unwrap().to_owned();
        assert_eq!(status.name(), "failed");
        assert!(
            reason.starts_with("no `Done` in the log within 1s"),
            "{reason}"
        );
        assert!(reason.ends_with("loading"), "{reason}");
    }

    #[test]
    fn last_lines_keeps_the_tail() {
        let text: String = (1..=25).map(|i| format!("l{i}\n")).collect();
        let tail = last_lines(&text);
        assert!(tail.starts_with("l6\n") && tail.ends_with("l25"), "{tail}");
    }
}
