use anyhow::{Context, Result};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::sync::mpsc as std_mpsc;
use tokio::sync::mpsc as tokio_mpsc;

/// Events emitted from the PTY reader thread to the TUI event loop.
pub enum PtyEvent {
    /// Raw bytes read from the PTY master (may contain ANSI escape codes).
    Data(Vec<u8>),
    /// The child process has exited. 0 = success, non-zero = failure.
    Exit(i32),
}

/// A live PTY session wrapping a child process.
///
/// The master PTY is held here for resize operations. A background thread
/// reads from it and forwards events through the `event_rx` channel.
/// A second background thread writes keypresses forwarded from the TUI.
pub struct PtySession {
    master: Box<dyn portable_pty::MasterPty>,
    /// Send raw bytes to the child process's stdin via the PTY.
    input_tx: std_mpsc::SyncSender<Vec<u8>>,
}

impl PtySession {
    /// Spawns `program args` inside a PTY of the given size.
    ///
    /// Returns the session (held by the TUI for writes/resize) and a receiver
    /// for `PtyEvent`s (drained each tick of the event loop).
    pub fn spawn(
        program: &str,
        args: &[&str],
        size: PtySize,
    ) -> Result<(Self, std_mpsc::Receiver<PtyEvent>)> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(size).context("Failed to open PTY")?;

        let mut cmd = CommandBuilder::new(program);
        for arg in args {
            cmd.arg(arg);
        }

        // spawn_command returns Box<dyn Child + Send>, so child is movable.
        let mut child = pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn command in PTY")?;

        // Clone the master reader before taking the writer (both require &self).
        let mut reader = pair
            .master
            .try_clone_reader()
            .context("Failed to clone PTY reader")?;

        let writer = pair
            .master
            .take_writer()
            .context("Failed to take PTY writer")?;

        // Channel from background threads → TUI event loop.
        let (event_tx, event_rx) = std_mpsc::sync_channel::<PtyEvent>(256);

        // Reader thread: read PTY output and forward to event channel.
        let reader_tx = event_tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if reader_tx.send(PtyEvent::Data(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // Wait thread: wait for child exit and forward exit code.
        std::thread::spawn(move || {
            let code = child
                .wait()
                .map(|s| if s.success() { 0 } else { 1 })
                .unwrap_or(1);
            let _ = event_tx.send(PtyEvent::Exit(code));
        });

        // Writer thread: receive bytes from TUI and write to PTY master.
        let (input_tx, input_rx) = std_mpsc::sync_channel::<Vec<u8>>(64);
        let mut writer: Box<dyn Write + Send> = writer;
        std::thread::spawn(move || {
            for bytes in input_rx {
                if writer.write_all(&bytes).is_err() {
                    break;
                }
                let _ = writer.flush();
            }
        });

        Ok((Self { master: pair.master, input_tx }, event_rx))
    }

    /// Forward raw bytes to the child process's stdin.
    pub fn write_bytes(&self, bytes: &[u8]) {
        let _ = self.input_tx.send(bytes.to_vec());
    }

    /// Notify the child process of a terminal resize.
    pub fn resize(&self, size: PtySize) {
        let _ = self.master.resize(size);
    }
}

/// Spawn a non-PTY async task for commands that produce plain text output (init, ready).
///
/// The task runs `f`, sends its output lines through `output_tx`, and sends the
/// exit code (0 on success, 1 on error) through `exit_tx` when done.
pub fn spawn_text_command<F, Fut>(
    output_tx: tokio_mpsc::UnboundedSender<String>,
    exit_tx: tokio::sync::oneshot::Sender<i32>,
    f: F,
) where
    F: FnOnce(crate::commands::output::OutputSink) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
{
    tokio::spawn(async move {
        let sink = crate::commands::output::OutputSink::Channel(output_tx);
        let code = match f(sink).await {
            Ok(()) => 0,
            Err(e) => {
                // The error message was not printed by the command — send it now.
                // (The sink was consumed so we can't use it here; the error is
                //  surfaced via the exit code and the TUI shows a generic message.)
                eprintln!("command error: {}", e);
                1
            }
        };
        let _ = exit_tx.send(code);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_spawn_runs_echo() {
        // Spawn a simple process through the PTY and verify we get output.
        let size = PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        };
        let (session, event_rx) = PtySession::spawn("echo", &["hello from pty"], size).unwrap();

        let mut received_data = false;
        let mut exit_code = None;

        // Collect events with a reasonable timeout. Continue draining after
        // Exit because Data events may arrive in the channel before Exit but
        // be polled after it (race between reader and wait threads).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            match event_rx.try_recv() {
                Ok(PtyEvent::Data(bytes)) => {
                    let text = String::from_utf8_lossy(&bytes);
                    if text.contains("hello from pty") {
                        received_data = true;
                    }
                }
                Ok(PtyEvent::Exit(code)) => {
                    exit_code = Some(code);
                    // Give the reader thread a moment to flush any in-flight data
                    // before draining: the wait thread and reader thread race.
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    // Drain any remaining Data events already in the channel.
                    while let Ok(PtyEvent::Data(bytes)) = event_rx.try_recv() {
                        let text = String::from_utf8_lossy(&bytes);
                        if text.contains("hello from pty") {
                            received_data = true;
                        }
                    }
                    break;
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }

        drop(session);
        assert!(received_data, "Expected output from echo command");
        assert_eq!(exit_code, Some(0), "Expected clean exit");
    }

    #[test]
    fn pty_exit_nonzero_on_failed_command() {
        let size = PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 };
        let (session, event_rx) =
            PtySession::spawn("sh", &["-c", "exit 42"], size).unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut exit_code = None;
        while std::time::Instant::now() < deadline {
            match event_rx.try_recv() {
                Ok(PtyEvent::Exit(code)) => {
                    exit_code = Some(code);
                    break;
                }
                _ => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }
        drop(session);
        // portable-pty maps non-zero exit to success=false → we map that to exit code 1.
        assert_eq!(exit_code, Some(1));
    }
}
