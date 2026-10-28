//! The child process a client drives: spawn, the writer task that owns its stdin, shutdown.

use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// After stdin closes, the process gets this long to finish (Claude Code writes its transcript) before SIGTERM, and again before SIGKILL.
const GRACE: Duration = Duration::from_secs(5);

/// One child process in a process group of its own. Dropping it kills the group.
pub(crate) struct Proc {
    child: Option<Child>,
    pid: Option<u32>,
    /// Cancelled on close: the writer lets go of stdin, which tells the process to finish.
    stop: CancellationToken,
}

impl Proc {
    /// Start cmd with stdin and stdout piped; each line sent to the returned sender goes to its stdin.
    pub(crate) fn spawn(cmd: &mut Command) -> std::io::Result<(Proc, mpsc::UnboundedSender<String>, ChildStdout)> {
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            // Its own process group: a stop reaches the commands it runs (Bash commands are its children).
            .process_group(0);
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (out, out_rx) = mpsc::unbounded_channel();
        let stop = CancellationToken::new();
        tokio::spawn(write(stdin, out_rx, stop.clone()));
        Ok((Proc { pid: child.id(), child: Some(child), stop }, out, stdout))
    }

    /// The process id from spawn time; None when it was gone before that.
    pub(crate) fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Close stdin and give the process time to finish, then SIGTERM its process group, then SIGKILL.
    /// Returns how it exited.
    pub(crate) async fn close(&mut self) -> Option<ExitStatus> {
        self.stop.cancel();
        let mut child = self.child.take()?;
        if let Ok(Ok(s)) = tokio::time::timeout(GRACE, child.wait()).await {
            return Some(s);
        }
        self.signal(libc::SIGTERM);
        if let Ok(Ok(s)) = tokio::time::timeout(GRACE, child.wait()).await {
            return Some(s);
        }
        self.signal(libc::SIGKILL);
        child.wait().await.ok()
    }

    fn signal(&self, sig: i32) {
        if let Some(pid) = self.pid {
            // SAFETY: killpg only sends a signal; the group is the child's own (process_group(0)).
            unsafe {
                libc::killpg(pid as i32, sig);
            }
        }
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        // kill_on_drop ends the child itself; the commands it runs get the group's SIGKILL.
        self.stop.cancel();
        if self.child.is_some() {
            self.signal(libc::SIGKILL);
        }
    }
}

async fn write(mut stdin: ChildStdin, mut rx: mpsc::UnboundedReceiver<String>, stop: CancellationToken) {
    loop {
        let line = tokio::select! {
            _ = stop.cancelled() => break,
            line = rx.recv() => match line { Some(l) => l, None => break },
        };
        if stdin.write_all(line.as_bytes()).await.is_err()
            || stdin.write_all(b"\n").await.is_err()
            || stdin.flush().await.is_err()
        {
            break;
        }
    }
    // Dropping stdin is the process's EOF.
}
