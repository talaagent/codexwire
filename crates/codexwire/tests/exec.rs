//! `exec::Exec::run` against a fake `codex` (`tests/fake_codex_exec`, sh):
//! the private home, the image in and out, the login copied back, the
//! failure, the timeout, and the run's directory removed.

// `#[tokio::test]` expands to `block_on`.
#![allow(clippy::disallowed_methods)]

use core::time::Duration;

use codexwire::exec::{Exec, Failure, Input};

const FAKE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fake_codex_exec");

fn setup() -> (tempfile::TempDir, Exec) {
    let d = tempfile::tempdir().expect("tempdir");
    let real = d.path().join("codex-home");
    std::fs::create_dir_all(&real).expect("mkdir");
    std::fs::write(real.join("auth.json"), br#"{"tokens":"old"}"#).expect("write");
    std::fs::write(real.join("history.jsonl"), b"secret history").expect("write");
    let mut e = Exec::new(FAKE, &real, d.path().join("state/codex"));
    e.timeout = Duration::from_secs(20);
    e.env = vec![
        ("PATH".into(), std::env::var("PATH").unwrap_or_default()),
        (
            "REAL_CODEX_HOME".into(),
            real.to_string_lossy().into_owned(),
        ),
    ];
    (d, e)
}

fn left(e: &Exec) -> usize {
    std::fs::read_dir(&e.tmp_root).expect("read_dir").count()
}

#[tokio::test]
async fn runs_in_a_private_home() {
    let (_d, e) = setup();
    let input = Input {
        prompt: "make an image".into(),
        images: vec![("../ref-1.png".into(), b"\x89PNG\r\n\x1a\n0000".to_vec())],
    };
    let run = e.run(&input).await.unwrap();
    assert_eq!(
        run.code,
        Some(0),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(run.last_message, b"done");
    let imgs = run.generated_images(1 << 20).await;
    assert_eq!(imgs.len(), 1);
    assert_eq!(std::fs::read(&imgs[0]).unwrap(), b"\x89PNG\r\n\x1a\n0000");
    assert_eq!(
        run.generated_images(4).await,
        Vec::<std::path::PathBuf>::new(),
        "too big"
    );
    // The refreshed login is back.
    assert!(*run.copied_back.as_ref().unwrap());
    assert_eq!(
        std::fs::read(e.codex_home.join("auth.json")).unwrap(),
        br#"{"tokens":"new"}"#
    );
    assert_eq!(left(&e), 1);
    run.remove().await.unwrap();
    assert_eq!(left(&e), 0);
}

#[tokio::test]
async fn says_why_it_failed() {
    let (_d, e) = setup();
    let run = e
        .run(&Input {
            prompt: "fail".into(),
            images: vec![],
        })
        .await
        .unwrap();
    assert_eq!(run.code, Some(3));
    assert_eq!(
        run.failure(),
        Some(Failure::Said("You've hit your usage limit.".into()))
    );
    assert!(!*run.copied_back.as_ref().unwrap(), "not refreshed");
    assert_eq!(
        std::fs::read(e.codex_home.join("auth.json")).unwrap(),
        br#"{"tokens":"old"}"#
    );
    drop(run);
    assert_eq!(left(&e), 0, "dropped: removed");
}

#[tokio::test]
async fn times_out() {
    let (_d, mut e) = setup();
    e.timeout = Duration::from_millis(300);
    let run = e
        .run(&Input {
            prompt: "hang".into(),
            images: vec![],
        })
        .await
        .unwrap();
    assert_eq!(run.code, None);
}

#[tokio::test]
async fn needs_a_login() {
    let (_d, mut e) = setup();
    e.codex_home = e.codex_home.join("missing");
    let err = e.run(&Input::default()).await.unwrap_err();
    assert!(matches!(err, codexwire::exec::ExecError::Login(_)), "{err}");
}
