//! Configuration reload on `SIGHUP` (`systemctl reload brickbot`).
//!
//! The configuration is immutable for the life of a process: it is cloned
//! into the Discord handler and every polling loop at startup. A reload
//! therefore validates the new file and, only if it is good, replaces the
//! process image with a fresh one — same PID, so systemd sees an
//! uninterrupted service. A bad file is logged and changes nothing.

use crate::config::Config;
use std::ffi::OsString;
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Resolves once a reload request arrives *and* `load` accepts the new
/// configuration. Rejected requests are logged and waited past, so a typo in
/// the file never takes the running bot down.
///
/// Never resolves if the request source closes, which leaves the caller's
/// `select!` to its other branches.
pub async fn wait_for_valid_reload<L>(mut requests: mpsc::Receiver<()>, load: L) -> Config
where
    L: Fn() -> anyhow::Result<Config>,
{
    loop {
        if requests.recv().await.is_none() {
            return std::future::pending().await;
        }
        tracing::info!("Configuration reload requested.");
        match load() {
            Ok(config) => return config,
            Err(e) => tracing::error!("Reload rejected, keeping the current configuration: {e}"),
        }
    }
}

/// The program and arguments to re-execute: this process's own command line.
///
/// `argv[0]` is preferred over the resolved executable so that a binary
/// replaced on disk at the same path is picked up by the reload, rather than
/// the deleted inode the running process still maps.
pub fn exec_target(
    args: impl IntoIterator<Item = OsString>,
    fallback_exe: PathBuf,
) -> (OsString, Vec<OsString>) {
    let mut args = args.into_iter();
    let program = args.next().unwrap_or_else(|| fallback_exe.into_os_string());
    (program, args.collect())
}

/// Turns `SIGHUP` into reload requests. Installing this also replaces the
/// signal's default action, which would otherwise terminate the process.
pub fn spawn_sighup_listener() -> std::io::Result<mpsc::Receiver<()>> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut hangup = signal(SignalKind::hangup())?;
    // One slot is enough: requests arriving while one is pending collapse
    // into it, since a reload always reads the file as it is now.
    let (tx, rx) = mpsc::channel(1);
    tokio::spawn(async move {
        while hangup.recv().await.is_some() {
            if tx.try_send(()) == Err(mpsc::error::TrySendError::Closed(())) {
                break;
            }
        }
    });
    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn os(parts: &[&str]) -> Vec<OsString> {
        parts.iter().map(OsString::from).collect()
    }

    #[tokio::test]
    async fn a_valid_config_resolves_with_the_new_config() {
        let (tx, rx) = mpsc::channel(1);
        tx.send(()).await.unwrap();

        let config = wait_for_valid_reload(rx, || {
            Ok(Config {
                poll_interval: Some(42),
                ..Config::default()
            })
        })
        .await;

        assert_eq!(config.poll_interval, Some(42));
    }

    #[tokio::test]
    async fn an_invalid_config_keeps_waiting_until_a_valid_one_arrives() {
        let (tx, rx) = mpsc::channel(4);
        let attempts = AtomicUsize::new(0);
        let load = || {
            if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                Err(anyhow::anyhow!("guild_id must be non-zero"))
            } else {
                Ok(Config::default())
            }
        };
        let mut waiting = std::pin::pin!(wait_for_valid_reload(rx, load));

        tx.send(()).await.unwrap();
        let rejected = tokio::time::timeout(Duration::from_millis(50), &mut waiting).await;
        assert!(rejected.is_err(), "a rejected reload must not resolve");

        tx.send(()).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), &mut waiting)
            .await
            .expect("the corrected config should be accepted");
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn nothing_is_loaded_before_a_request() {
        let (_tx, rx) = mpsc::channel::<()>(1);
        let attempts = AtomicUsize::new(0);
        let load = || {
            attempts.fetch_add(1, Ordering::SeqCst);
            Ok(Config::default())
        };

        let waited =
            tokio::time::timeout(Duration::from_millis(50), wait_for_valid_reload(rx, load)).await;

        assert!(waited.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_closed_request_source_never_resolves() {
        let (tx, rx) = mpsc::channel::<()>(1);
        drop(tx);

        let waited = tokio::time::timeout(
            Duration::from_millis(50),
            wait_for_valid_reload(rx, || Ok(Config::default())),
        )
        .await;

        assert!(waited.is_err());
    }

    #[test]
    fn exec_target_keeps_the_command_line_in_order() {
        let (program, args) = exec_target(
            os(&[
                "/opt/brickbot/brickbot",
                "--config",
                "other.toml",
                "--build-debug-log",
                "data/build.log",
            ]),
            PathBuf::from("/proc/self/exe"),
        );

        assert_eq!(program, OsString::from("/opt/brickbot/brickbot"));
        assert_eq!(
            args,
            os(&[
                "--config",
                "other.toml",
                "--build-debug-log",
                "data/build.log"
            ])
        );
    }

    #[test]
    fn exec_target_falls_back_to_the_executable_when_argv_is_empty() {
        let (program, args) = exec_target(os(&[]), PathBuf::from("/opt/brickbot/brickbot"));

        assert_eq!(program, OsString::from("/opt/brickbot/brickbot"));
        assert!(args.is_empty());
    }
}
