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
    /// The commands it left running in its group get the SIGTERM too. Returns how it exited.
    pub(crate) async fn close(&mut self) -> Option<ExitStatus> {
        self.stop.cancel();
        // The child stays in self.child until it is reaped: a close dropped halfway still kills the group on Drop.
        let child = self.child.as_mut()?;
        let mut status = tokio::time::timeout(GRACE, child.wait()).await.ok().and_then(Result::ok);
        signal(self.pid, libc::SIGTERM);
        if status.is_none() {
            status = tokio::time::timeout(GRACE, child.wait()).await.ok().and_then(Result::ok);
        }
        if status.is_none() {
            signal(self.pid, libc::SIGKILL);
            status = child.wait().await.ok();
        }
        self.child = None;
        status
    }
}

fn signal(pid: Option<u32>, sig: i32) {
    if let Some(pid) = pid {
        // SAFETY: killpg only sends a signal; the group is the child's own (process_group(0)).
        unsafe {
            libc::killpg(pid as i32, sig);
        }
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        // kill_on_drop ends the child itself; the commands it runs get the group's SIGKILL.
        self.stop.cancel();
        if self.child.is_some() {
            signal(self.pid, libc::SIGKILL);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether any process of the group is left, waiting up to 3 s for it to go.
    async fn group_alive(pid: u32) -> bool {
        for _ in 0..60 {
            // SAFETY: signal 0 only checks that the group exists.
            if unsafe { libc::killpg(pid as i32, 0) } != 0 {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        true
    }

    #[tokio::test]
    async fn a_clean_exit_ends_its_background_commands() {
        let (mut p, _tx, _out) = Proc::spawn(Command::new("sh").args(["-c", "sleep 30 & exec cat"])).unwrap();
        let pid = p.pid().unwrap();
        assert!(p.close().await.is_some_and(|s| s.success()));
        assert!(!group_alive(pid).await);
    }

    #[tokio::test]
    async fn a_close_cut_short_still_kills_the_group() {
        let (mut p, _tx, _out) = Proc::spawn(Command::new("sh").args(["-c", "sleep 30 & sleep 30"])).unwrap();
        let pid = p.pid().unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(200), p.close()).await.is_err());
        drop(p);
        assert!(!group_alive(pid).await);
    }
}
