//! Per-worker cancellation and diagnostics for synchronous render helpers.
//! The scope is installed by `render` and restored on every return/unwind. It
//! deliberately does not affect unrelated child processes or other threads.
use super::CancellationToken;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Clone)]
struct Context {
    cancellation: CancellationToken,
    log_path: PathBuf,
}

thread_local! {
    static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) };
}

pub(super) struct Scope(Option<Context>);

impl Scope {
    pub(super) fn enter(cancellation: CancellationToken, log_path: PathBuf) -> io::Result<Self> {
        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent)?;
        }
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&log_path)?;
        Ok(Self(CONTEXT.with(|context| {
            context.replace(Some(Context {
                cancellation,
                log_path,
            }))
        })))
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        CONTEXT.with(|context| context.replace(self.0.take()));
    }
}

pub(super) fn check_cancelled() -> io::Result<()> {
    let cancelled = CONTEXT.with(|context| {
        context
            .borrow()
            .as_ref()
            .is_some_and(|context| context.cancellation.is_cancelled())
    });
    if cancelled {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "成片任务已取消。",
        ))
    } else {
        Ok(())
    }
}

pub(super) fn record(event: &str, details: Value) -> io::Result<()> {
    CONTEXT.with(|context| {
        if let Some(context) = context.borrow().as_ref() {
            let mut file = OpenOptions::new().append(true).open(&context.log_path)?;
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            serde_json::to_writer(
                &mut file,
                &json!({
                    "unixMilliseconds": timestamp, "event": event, "details": details,
                }),
            )?;
            file.write_all(b"\n")?;
            file.flush()?;
        }
        Ok(())
    })
}

// Killing/waiting only the handle created by this runner preserves ownership.
// The guard also covers spawn/read setup errors and unwinding.
struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        finish_owned_child(&mut self.0);
    }
}

fn finish_owned_child(child: &mut Child) -> ExitStatus {
    let pid = child.id();
    let mut confirmed_status = None;
    super::wait_for_verified_cleanup(
        || {
            match child.try_wait() {
                Ok(Some(status)) => {
                    confirmed_status = Some(status);
                    return None;
                }
                Ok(None) => {}
                Err(error) => return Some(format!("无法核对本任务媒体句柄：{error}")),
            }
            match child.kill().and_then(|_| child.wait()) {
                Ok(status) => {
                    confirmed_status = Some(status);
                    None
                }
                Err(error) => Some(format!("本任务媒体进程尚未确认停止：{error}")),
            }
        },
        |error| {
            let _ = record("media_cleanup_pending", json!({"pid": pid, "error": error}));
        },
        || thread::sleep(Duration::from_secs(2)),
    );
    confirmed_status.expect("cleanup returns only after this Child's exit is confirmed")
}

fn drain(mut stream: impl Read) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut truncated = false;
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        // Never stop draining: a full pipe would deadlock the child. Preserve
        // the tail for error diagnostics while bounding memory use.
        if output.len() + count > OUTPUT_LIMIT {
            truncated = true;
            let remove = (output.len() + count - OUTPUT_LIMIT).min(output.len());
            output.drain(..remove);
        }
        output.extend_from_slice(&chunk[..count]);
    }
    if truncated {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "媒体工具诊断输出超过上限，不能把截断日志当作完整质检结果。",
        ))
    } else {
        Ok(output)
    }
}

pub(super) fn output(command: Command) -> io::Result<Output> {
    controlled_output(command, COMMAND_TIMEOUT, check_cancelled)
}

pub(super) fn cleanup_output(command: Command) -> io::Result<Output> {
    controlled_output(command, Duration::from_secs(5), || Ok(()))
}

fn controlled_output(
    mut command: Command,
    timeout: Duration,
    mut cancelled: impl FnMut() -> io::Result<()>,
) -> io::Result<Output> {
    cancelled()?;
    record(
        "media_start",
        json!({
            "program": command.get_program().to_string_lossy(),
            "arguments": command.get_args().map(|value| value.to_string_lossy().into_owned()).collect::<Vec<_>>(),
            "timeoutSeconds": timeout.as_secs(),
        }),
    )?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let started = Instant::now();
    let mut child = OwnedChild(command.spawn()?);
    let pid = child.0.id();
    let stdout = child
        .0
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing stdout pipe"))?;
    let stderr = child
        .0
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("missing stderr pipe"))?;
    let out_reader = thread::spawn(move || drain(stdout));
    let err_reader = thread::spawn(move || drain(stderr));
    let mut failure = None;
    let status = loop {
        match child.0.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => failure = Some(error),
        }
        if failure.is_none() {
            if let Err(error) = cancelled() {
                failure = Some(error);
            } else if started.elapsed() >= timeout {
                failure = Some(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "媒体处理超时，已停止本任务的子进程。",
                ));
            }
        }
        if failure.is_some() {
            // Do not release the render activity permit on a failed kill.
            // Cancellation stops production, not cleanup ownership.
            break finish_owned_child(&mut child.0);
        }
        thread::sleep(Duration::from_millis(50));
    };
    let stdout = out_reader
        .join()
        .map_err(|_| io::Error::other("stdout reader panicked"));
    let stderr = err_reader
        .join()
        .map_err(|_| io::Error::other("stderr reader panicked"));
    // Join both consumers before propagating either one's I/O error.
    let stdout = stdout??;
    let stderr = stderr??;
    record(
        "media_process",
        json!({
            "pid": pid, "elapsedMilliseconds": started.elapsed().as_millis(),
            "exitCode": status.code(), "failure": failure.as_ref().map(ToString::to_string),
            "stderrTail": String::from_utf8_lossy(&stderr).lines().rev().take(12).collect::<Vec<_>>(),
        }),
    )?;
    if let Some(error) = failure {
        return Err(error);
    }
    cancelled()?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // The child is this exact test binary. No shell, game or network service is
    // involved; the environment flag enables only the isolated sleeping test.
    #[test]
    fn controlled_child_fixture() {
        if std::env::var_os("D2H_CONTROLLED_CHILD_FIXTURE").is_some() {
            thread::sleep(Duration::from_secs(30));
        }
    }

    fn sleeping_test_child() -> Command {
        let mut command = super::super::background_command(std::env::current_exe().unwrap());
        command.args([
            "--exact",
            "execution::tests::controlled_child_fixture",
            "--nocapture",
        ]);
        command.env("D2H_CONTROLLED_CHILD_FIXTURE", "1");
        command
    }

    #[test]
    fn media_timeout_terminates_and_reaps_its_child() {
        let started = Instant::now();
        let result =
            controlled_output(sleeping_test_child(), Duration::from_millis(200), || Ok(()));
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn media_cancellation_terminates_and_reaps_its_child() {
        let started = Instant::now();
        let result = controlled_output(sleeping_test_child(), Duration::from_secs(20), || {
            if started.elapsed() >= Duration::from_millis(200) {
                Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "cancelled fixture",
                ))
            } else {
                Ok(())
            }
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
