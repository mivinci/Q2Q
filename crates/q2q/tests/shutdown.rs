use super::*;

#[tokio::test]
async fn successful_initialization_does_not_consume_a_pending_stop() {
  let mut stop: Stop = Box::pin(std::future::pending());
  let (value, interrupted) = initialize(async { Ok(42) }, &mut stop).await.unwrap();
  assert_eq!(value, 42);
  assert!(!interrupted);
}

#[tokio::test]
async fn interrupt_waits_for_initialization_and_returns_its_handle() {
  let (seen, signal_seen) = tokio::sync::oneshot::channel();
  let (release, ready) = tokio::sync::oneshot::channel();
  let task = tokio::spawn(async move {
    let mut stop: Stop = Box::pin(async {
      seen.send(()).unwrap();
      Ok(())
    });
    initialize(
      async {
        ready.await?;
        Ok(42)
      },
      &mut stop,
    )
    .await
  });
  signal_seen.await.unwrap();
  assert!(!task.is_finished(), "interrupt abandoned initialization");
  release.send(()).unwrap();
  let (value, interrupted) = task.await.unwrap().unwrap();
  assert_eq!(value, 42);
  assert!(interrupted);
}

#[tokio::test]
async fn initialization_failure_after_interrupt_preserves_error() {
  let mut stop: Stop = Box::pin(async { Ok(()) });
  let error = initialize(
    async {
      tokio::task::yield_now().await;
      Err::<(), _>(io::Error::other("initialization failed").into())
    },
    &mut stop,
  )
  .await
  .unwrap_err();
  assert_eq!(error.to_string(), "initialization failed");
}

#[tokio::test]
async fn signal_registration_failure_is_reported() {
  let mut stop: Stop = Box::pin(async { Err(io::Error::other("signal failed")) });
  let error = initialize(std::future::pending::<crate::Result<()>>(), &mut stop)
    .await
    .unwrap_err();
  assert_eq!(error.to_string(), "signal failed");
}
