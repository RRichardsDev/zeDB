use std::io::{Read, Write};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_CHILD_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

/// Run a child with captured output and an optional stdin body. A timed-out
/// child is killed and reaped before this function returns.
pub(crate) fn output_with_timeout(
    mut command: Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
) -> std::io::Result<Output> {
    output_with_limits(&mut command, input, timeout, MAX_CHILD_OUTPUT_BYTES)
}

fn output_with_limits(
    command: &mut Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
    output_limit: usize,
) -> std::io::Result<Output> {
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let mut child = command.spawn()?;

    let writer = input.map(|input| {
        let mut stdin = child.stdin.take().expect("child stdin was piped");
        std::thread::spawn(move || stdin.write_all(&input))
    });
    let exceeded = Arc::new(AtomicBool::new(false));
    let stdout = child.stdout.take().expect("child stdout was piped");
    let stdout_exceeded = Arc::clone(&exceeded);
    let stdout_reader =
        std::thread::spawn(move || read_bounded(stdout, output_limit, stdout_exceeded));
    let stderr = child.stderr.take().expect("child stderr was piped");
    let stderr_exceeded = Arc::clone(&exceeded);
    let stderr_reader =
        std::thread::spawn(move || read_bounded(stderr, output_limit, stderr_exceeded));

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                terminate_process_group(child.id());
                break status;
            }
            Ok(None) => {}
            Err(error) => {
                terminate_and_reap(&mut child);
                return Err(error);
            }
        }
        if exceeded.load(Ordering::Relaxed) || Instant::now() >= deadline {
            terminate_and_reap(&mut child);
            if let Some(writer) = writer {
                let _ = writer.join();
            }
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            if exceeded.load(Ordering::Relaxed) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("child process output exceeded {output_limit} bytes"),
                ));
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "child process exceeded {} second deadline",
                    timeout.as_secs()
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    if let Some(writer) = writer {
        writer
            .join()
            .map_err(|_| std::io::Error::other("child stdin writer panicked"))??;
    }
    let stdout = stdout_reader
        .join()
        .map_err(|_| std::io::Error::other("child stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| std::io::Error::other("child stderr reader panicked"))??;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn read_bounded(
    mut reader: impl Read,
    limit: usize,
    exceeded: Arc<AtomicBool>,
) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(bytes);
        }
        if bytes
            .len()
            .checked_add(read)
            .is_none_or(|next| next > limit)
        {
            exceeded.store(true, Ordering::Relaxed);
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "captured child output exceeded its safety limit",
            ));
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
}

fn terminate_and_reap(child: &mut std::process::Child) {
    terminate_process_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

fn terminate_process_group(child_id: u32) {
    #[cfg(unix)]
    {
        if let Ok(process_group) = i32::try_from(child_id) {
            // The child is the leader of the process group configured above.
            unsafe {
                libc::kill(-process_group, libc::SIGKILL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn kills_and_reaps_a_nonterminating_child() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 5"]);
        let started = Instant::now();
        let error = output_with_timeout(command, None, Duration::from_millis(100)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_unbounded_child_output() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "yes x"]);
        let error =
            output_with_limits(&mut command, None, Duration::from_secs(2), 1024).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[cfg(unix)]
    #[test]
    fn reaps_background_descendants_that_inherit_output_pipes() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 5 & exit 0"]);
        let started = Instant::now();
        let output = output_with_limits(&mut command, None, Duration::from_secs(2), 1024).unwrap();
        assert!(output.status.success());
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
