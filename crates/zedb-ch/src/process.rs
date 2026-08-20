use std::io::{Read, Write};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Run a child with captured output and an optional stdin body. A timed-out
/// child is killed and reaped before this function returns.
pub(crate) fn output_with_timeout(
    mut command: Command,
    input: Option<Vec<u8>>,
    timeout: Duration,
) -> std::io::Result<Output> {
    command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;

    let writer = input.map(|input| {
        let mut stdin = child.stdin.take().expect("child stdin was piped");
        std::thread::spawn(move || stdin.write_all(&input))
    });
    let mut stdout = child.stdout.take().expect("child stdout was piped");
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let mut stderr = child.stderr.take().expect("child stderr was piped");
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(writer) = writer {
                let _ = writer.join();
            }
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
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
}
