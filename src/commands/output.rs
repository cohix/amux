use std::io::Write;
use tokio::sync::mpsc::UnboundedSender;

/// Routes command output to either stdout (command mode) or a TUI channel (interactive mode).
///
/// This abstraction lets every command function work identically in both execution
/// contexts without duplicating logic.
pub enum OutputSink {
    Stdout,
    Channel(UnboundedSender<String>),
}

impl OutputSink {
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
}
