//! Tokio runtime on its own thread, connected to the UI through `async-channel`.
//!
//! The GTK thread only touches the two channels: it sends `Command` without
//! blocking and reads `Event` from the GLib loop. No `block_on` over there.

use std::io;
use std::thread::{self, JoinHandle};

use crate::orchestrator::{self, Command, Event};

pub struct Runtime {
    pub commands: async_channel::Sender<Command>,
    pub events: async_channel::Receiver<Event>,
    thread: JoinHandle<()>,
}

impl Runtime {
    pub fn start() -> io::Result<Self> {
        Self::start_with(orchestrator::Config::default())
    }

    pub fn start_with(config: orchestrator::Config) -> io::Result<Self> {
        let (commands, rx_commands) = async_channel::unbounded();
        let (tx_events, events) = async_channel::unbounded();

        let rt = tokio::runtime::Builder::new_multi_thread().enable_all().thread_name("orch-tokio").build()?;
        let thread = thread::Builder::new()
            .name("orch-runtime".into())
            .spawn(move || rt.block_on(orchestrator::run_with(config, rx_commands, tx_events)))?;

        Ok(Self { commands, events, thread })
    }

    /// Asks the orchestrator to stop and waits for the thread to finish.
    pub fn shutdown(self) {
        let _ = self.commands.send_blocking(Command::Shutdown);
        drop(self.commands);
        if self.thread.join().is_err() {
            tracing::error!("runtime thread panicked");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_event_reaches_another_thread() {
        let config = orchestrator::Config { store: None, voice: false, discover: false, ..Default::default() };
        let runtime = Runtime::start_with(config).unwrap();
        assert_ne!(thread::current().name(), Some("orch-runtime"));

        assert!(matches!(runtime.events.recv_blocking().unwrap(), Event::State(_)));
        runtime.commands.send_blocking(Command::Ping).unwrap();
        assert!(matches!(runtime.events.recv_blocking().unwrap(), Event::Pong { .. }));

        runtime.shutdown();
    }
}
