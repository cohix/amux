use std::io::Write;
use tokio::sync::mpsc::UnboundedSender;

/// Routes command output to either stdout (command mode) or a TUI channel (interactive mode).
///
/// This abstraction lets every command function work identically in both execution
/// contexts without duplicating logic.
#[derive(Clone)]
pub enum OutputSink {
    Stdout,
    Channel(UnboundedSender<String>),
}

impl OutputSink {
    /// Returns true when the sink writes directly to a terminal (stdout),
    /// enabling ANSI colour output. Returns false for TUI channel sinks.
    pub fn supports_color(&self) -> bool {
        matches!(self, OutputSink::Stdout)
    }

    pub fn println(&self, s: impl Into<String>) {
        match self {
            OutputSink::Stdout => println!("{}", s.into()),
            OutputSink::Channel(tx) => {
                let _ = tx.send(s.into());
            }
        }
    }

    pub fn print(&self, s: impl Into<String>) {
        match self {
            OutputSink::Stdout => {
                print!("{}", s.into());
                let _ = std::io::stdout().flush();
            }
            OutputSink::Channel(tx) => {
                let _ = tx.send(s.into());
            }
        }
    }

    /// Send a line, returning `true` on success.
    ///
    /// For `Stdout`, always succeeds. For `Channel`, returns `false` when the
    /// receiver has been dropped (e.g. the TUI tab was replaced by a new command).
    /// Callers can use this to detect channel closure and terminate watch loops.
    pub fn try_println(&self, s: impl Into<String>) -> bool {
        match self {
            OutputSink::Stdout => {
                println!("{}", s.into());
                true
            }
            OutputSink::Channel(tx) => tx.send(s.into()).is_ok(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn channel_sink_delivers_messages() {
        let (tx, mut rx) = unbounded_channel();
        let sink = OutputSink::Channel(tx);
        sink.println("hello");
        sink.println("world");
        assert_eq!(rx.try_recv().unwrap(), "hello");
        assert_eq!(rx.try_recv().unwrap(), "world");
    }

    #[test]
    fn try_println_returns_true_for_open_channel() {
        let (tx, mut rx) = unbounded_channel();
        let sink = OutputSink::Channel(tx);
        assert!(sink.try_println("msg"));
        assert_eq!(rx.try_recv().unwrap(), "msg");
    }

    #[test]
    fn try_println_returns_false_for_dropped_receiver() {
        let (tx, rx) = unbounded_channel::<String>();
        drop(rx);
        let sink = OutputSink::Channel(tx);
        assert!(!sink.try_println("msg"));
    }
}
