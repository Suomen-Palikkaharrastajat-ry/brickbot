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

/// The command lines to re-execute, in the order to try them: this
/// process's own, then the same arguments on the running executable.
///
/// `argv[0]` comes first so that a binary replaced on disk at the same path
/// is picked up by the reload. `current_exe` is the way back when that path
/// no longer resolves — by then the bot has shut down, so failing to start
/// anything would turn a reload into an outage.
pub fn exec_targets(
    args: impl IntoIterator<Item = OsString>,
    current_exe: Option<PathBuf>,
) -> Vec<(OsString, Vec<OsString>)> {
    let mut args = args.into_iter();
    let program = args.next();
    let args: Vec<OsString> = args.collect();

    let fallback = current_exe
        .map(PathBuf::into_os_string)
        .filter(|exe| program.as_ref() != Some(exe));
    program
        .into_iter()
        .chain(fallback)
        .map(|program| (program, args.clone()))
        .collect()
}

/// Turns `SIGHUP` into reload requests.
///
/// Installing this also replaces the signal's default action, which would
/// otherwise terminate the process — so call it first thing: until then a
/// reload request is a kill.
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
    fn exec_targets_keep_the_command_line_in_order() {
        let targets = exec_targets(
            os(&[
                "/opt/brickbot/brickbot",
                "--config",
                "other.toml",
                "--build-debug-log",
                "data/build.log",
            ]),
            Some(PathBuf::from("/opt/brickbot/brickbot")),
        );

        assert_eq!(
            targets,
            vec![(
                OsString::from("/opt/brickbot/brickbot"),
                os(&[
                    "--config",
                    "other.toml",
                    "--build-debug-log",
                    "data/build.log"
                ])
            )]
        );
    }

    #[test]
    fn exec_targets_fall_back_to_the_running_executable() {
        // `argv[0]` may no longer resolve: a bare name with a changed PATH,
        // or a binary caught halfway through being replaced.
        let targets = exec_targets(
            os(&["brickbot", "--config", "other.toml"]),
            Some(PathBuf::from("/nix/store/abc/bin/brickbot")),
        );

        assert_eq!(
            targets,
            vec![
                (OsString::from("brickbot"), os(&["--config", "other.toml"])),
                (
                    OsString::from("/nix/store/abc/bin/brickbot"),
                    os(&["--config", "other.toml"])
                ),
            ]
        );
    }

    #[test]
    fn exec_targets_use_the_executable_alone_when_argv_is_empty() {
        let targets = exec_targets(os(&[]), Some(PathBuf::from("/opt/brickbot/brickbot")));

        assert_eq!(
            targets,
            vec![(OsString::from("/opt/brickbot/brickbot"), vec![])]
        );
    }

    #[test]
    fn exec_targets_without_a_known_executable_still_try_argv() {
        let targets = exec_targets(os(&["brickbot"]), None);

        assert_eq!(targets, vec![(OsString::from("brickbot"), vec![])]);
    }
}
