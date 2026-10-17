use super::*;

fn message(kind: SignalType, id: &str) -> SignalRequest {
  SignalRequest {
    kind,
    connection_id: id.into(),
    connect_token: None,
    sdp: None,
    candidate: None,
    reason: None,
    accept_token: None,
    signal_key: None,
  }
}
fn fixture() -> (
  mpsc::Sender<Result<SignalRequest, ControlError>>,
  SignalRouter,
  mpsc::Receiver<String>,
) {
  let (tx, rx) = mpsc::channel::<Result<SignalRequest, ControlError>>(128);
  let rx = Arc::new(AsyncMutex::new(rx));
  let (rejected, rejects) = mpsc::channel(16);
  let router = SignalRouter::new(
    move || {
      let rx = rx.clone();
      async move {
        rx.lock()
          .await
          .recv()
          .await
          .unwrap_or_else(|| Err(ControlError::Transport("test input closed".into())))
      }
    },
    move |id| {
      let rejected = rejected.clone();
      async move {
        let _ = rejected.send(id).await;
      }
    },
  );
  (tx, router, rejects)
}

#[tokio::test]
async fn concurrent_connections_receive_only_their_own_signals() {
  let (tx, router, _) = fixture();
  tx.send(Ok(message(SignalType::Connect, "a")))
    .await
    .unwrap();
  tx.send(Ok(message(SignalType::Connect, "b")))
    .await
    .unwrap();
  let a = router.next().await.unwrap();
  let b = router.next().await.unwrap();
  tx.send(Ok(message(SignalType::Candidate, "b")))
    .await
    .unwrap();
  tx.send(Ok(message(SignalType::Offer, "a"))).await.unwrap();
  assert_eq!(a.route.recv().await.unwrap().kind, SignalType::Offer);
  assert_eq!(b.route.recv().await.unwrap().connection_id, "b");
  // Duplicate Connect must neither replace the mailbox nor admit a second task.
  tx.send(Ok(message(SignalType::Connect, "a")))
    .await
    .unwrap();
  tx.send(Ok(message(SignalType::Cancel, "a"))).await.unwrap();
  assert_eq!(a.route.recv().await.unwrap().kind, SignalType::Cancel);
  assert!(
    tokio::time::timeout(Duration::from_millis(10), router.next())
      .await
      .is_err()
  );
  drop(a);
  tx.send(Ok(message(SignalType::Offer, "a"))).await.unwrap();
  tx.send(Ok(message(SignalType::Candidate, "b")))
    .await
    .unwrap();
  assert_eq!(b.route.recv().await.unwrap().kind, SignalType::Candidate);
}

#[tokio::test]
async fn overflowing_one_mailbox_does_not_block_another_connection() {
  let (tx, router, _) = fixture();
  tx.send(Ok(message(SignalType::Connect, "a")))
    .await
    .unwrap();
  tx.send(Ok(message(SignalType::Connect, "b")))
    .await
    .unwrap();
  let a = router.next().await.unwrap();
  let b = router.next().await.unwrap();
  for _ in 0..=MAILBOX {
    tx.send(Ok(message(SignalType::Candidate, "a")))
      .await
      .unwrap();
  }
  tx.send(Ok(message(SignalType::Candidate, "b")))
    .await
    .unwrap();
  tokio::time::timeout(Duration::from_secs(1), b.route.recv())
    .await
    .unwrap()
    .unwrap();
  let error = a.route.recv().await.unwrap_err();
  assert!(
    matches!(error, ControlError::Shared(error) if matches!(&*error, ControlError::Protocol(message) if message.contains("overflow")))
  );
}

#[tokio::test]
async fn pending_admission_is_bounded_and_dropped_routes_are_reusable() {
  let (tx, router, mut rejects) = fixture();
  for index in 0..=INCOMING {
    tx.send(Ok(message(SignalType::Connect, &format!("peer-{index}"))))
      .await
      .unwrap();
  }
  assert_eq!(
    tokio::time::timeout(Duration::from_secs(1), rejects.recv())
      .await
      .unwrap()
      .unwrap(),
    format!("peer-{INCOMING}")
  );
  for _ in 0..INCOMING {
    drop(router.next().await.unwrap());
  }
  tx.send(Ok(message(SignalType::Connect, "fresh")))
    .await
    .unwrap();
  assert_eq!(router.next().await.unwrap().connect.connection_id, "fresh");
}

#[tokio::test]
async fn failure_preserves_the_original_cause_and_drop_wakes_sessions() {
  let (tx, router, _) = fixture();
  tx.send(Ok(message(SignalType::Connect, "a")))
    .await
    .unwrap();
  let a = router.next().await.unwrap();
  tx.send(Err(ControlError::Protocol("broken QUIC frame".into())))
    .await
    .unwrap();
  let source = match a.route.recv().await {
    Err(ControlError::Shared(error)) => error,
    _ => panic!("expected shared failure"),
  };
  assert!(matches!(&*source, ControlError::Protocol(message) if message == "broken QUIC frame"));
  assert!(
    matches!(router.next().await, Err(ControlError::Shared(error)) if Arc::ptr_eq(&source, &error))
  );
  let (tx, router, _) = fixture();
  tx.send(Ok(message(SignalType::Connect, "b")))
    .await
    .unwrap();
  let b = router.next().await.unwrap();
  drop(router);
  assert!(
    tokio::time::timeout(Duration::from_secs(1), b.route.recv())
      .await
      .unwrap()
      .is_err()
  );
}

#[tokio::test]
async fn slow_busy_responses_do_not_stop_existing_mailboxes() {
  let (tx, rx) = mpsc::channel::<Result<SignalRequest, ControlError>>(128);
  let rx = Arc::new(AsyncMutex::new(rx));
  let router = SignalRouter::new(
    move || {
      let rx = rx.clone();
      async move { rx.lock().await.recv().await.unwrap() }
    },
    |_| async { std::future::pending::<()>().await },
  );
  tx.send(Ok(message(SignalType::Connect, "active")))
    .await
    .unwrap();
  let active = router.next().await.unwrap();
  for index in 0..INCOMING + REJECTIONS + 2 {
    tx.send(Ok(message(SignalType::Connect, &format!("queued-{index}"))))
      .await
      .unwrap();
  }
  tx.send(Ok(message(SignalType::Candidate, "active")))
    .await
    .unwrap();
  assert_eq!(
    tokio::time::timeout(Duration::from_secs(1), active.route.recv())
      .await
      .unwrap()
      .unwrap()
      .kind,
    SignalType::Candidate
  );
}

#[tokio::test]
async fn paused_admission_rejects_buffered_and_new_requests_then_resumes() {
  let (sender, router, mut rejected) = fixture();
  sender
    .send(Ok(message(SignalType::Connect, "buffered")))
    .await
    .unwrap();
  tokio::task::yield_now().await;
  router.set_accepting(false).await;
  assert_eq!(rejected.recv().await.unwrap(), "buffered");
  sender
    .send(Ok(message(SignalType::Connect, "paused")))
    .await
    .unwrap();
  assert_eq!(rejected.recv().await.unwrap(), "paused");
  router.set_accepting(true).await;
  sender
    .send(Ok(message(SignalType::Connect, "resumed")))
    .await
    .unwrap();
  let request = router.next().await.unwrap();
  assert_eq!(request.connect.connection_id, "resumed");
}

#[tokio::test]
async fn pausing_wakes_receiver_and_lock_waiters_without_an_incoming_request() {
  let (_sender, router, _) = fixture();
  let router = Arc::new(router);
  let mut waiters = Vec::new();
  for _ in 0..2 {
    let router = router.clone();
    waiters.push(tokio::spawn(async move { router.next().await }));
    tokio::task::yield_now().await;
  }
  tokio::time::timeout(Duration::from_secs(1), router.set_accepting(false))
    .await
    .unwrap();
  for waiter in waiters {
    assert!(matches!(
      tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .unwrap()
        .unwrap(),
      Err(ControlError::AdmissionPaused)
    ));
  }
  router.set_accepting(true).await;
  let pending = router.next();
  assert!(
    tokio::time::timeout(Duration::from_millis(10), pending)
      .await
      .is_err()
  );
}
