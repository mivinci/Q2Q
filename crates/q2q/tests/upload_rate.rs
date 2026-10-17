use super::*;
use std::sync::{Arc, Mutex};

#[tokio::test(start_paused = true)]
async fn all_connections_share_one_budget_and_idle_credit_is_bounded() {
  let rate = Arc::new(UploadRate::new(1041));
  let start = Instant::now();
  let observed = Arc::new(Mutex::new(0usize));
  let worker = || {
    let rate = rate.clone();
    let observed = observed.clone();
    async move {
      for _ in 0..4 {
        rate.acquire(1041).await;
        let mut bytes = observed.lock().unwrap();
        *bytes += 1041;
        assert!(*bytes as f64 <= 1041.0 + start.elapsed().as_secs_f64() * 1041.0 + 1.0);
      }
    }
  };
  tokio::join!(worker(), worker());
  assert!(start.elapsed() >= Duration::from_secs(7));
  tokio::time::sleep(Duration::from_secs(3600)).await;
  rate.acquire(1041).await;
  let before = Instant::now();
  rate.acquire(1041).await;
  assert!(before.elapsed() >= Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn cancelled_wait_leaves_no_future_bandwidth_debt() {
  let rate = UploadRate::new(1);
  rate.acquire(1041).await;
  assert!(
    tokio::time::timeout(Duration::from_secs(1), rate.acquire(1041))
      .await
      .is_err()
  );
  tokio::time::sleep(Duration::from_secs(1040)).await;
  let now = Instant::now();
  rate.acquire(1041).await;
  assert_eq!(now.elapsed(), Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn short_piece_charges_its_actual_encoded_length() {
  let rate = UploadRate::new(1041);
  rate.acquire(1041).await;
  let now = Instant::now();
  rate.acquire(30).await;
  assert!(now.elapsed() >= Duration::from_secs_f64(30.0 / 1041.0));
  assert!(now.elapsed() < Duration::from_millis(40));
}

#[tokio::test(start_paused = true)]
async fn continuously_ready_connections_take_fifo_piece_turns() {
  for peers in [2, 4, 16] {
    for bytes_per_second in [1041, 10 * 1024 * 1024] {
      let rate = Arc::new(UploadRate::new(bytes_per_second));
      // Register all senders before granting the first turn. Check FIFO both
      // while waiting for tokens and while a large burst can grant immediately.
      let turn = rate.state.lock().await;
      let order = Arc::new(Mutex::new(Vec::new()));
      let mut workers = Vec::new();
      for id in 0..peers {
        let rate = rate.clone();
        let order = order.clone();
        workers.push(tokio::spawn(async move {
          for _ in 0..4 {
            rate.acquire(1041).await;
            order.lock().unwrap().push(id);
          }
        }));
        tokio::task::yield_now().await;
      }
      drop(turn);
      for worker in workers {
        worker.await.unwrap();
      }
      let expected: Vec<_> = (0..4).flat_map(|_| 0..peers).collect();
      assert_eq!(*order.lock().unwrap(), expected);
    }
  }
}

#[tokio::test(start_paused = true)]
async fn cancelling_owner_and_queued_waiter_preserves_order_and_budget() {
  let rate = Arc::new(UploadRate::new(1041));
  rate.acquire(1041).await;
  let start = Instant::now();
  let order = Arc::new(Mutex::new(Vec::new()));
  let spawn = |id| {
    let rate = rate.clone();
    let order = order.clone();
    tokio::spawn(async move {
      rate.acquire(1041).await;
      order.lock().unwrap().push(id);
    })
  };
  let owner = spawn(0);
  tokio::task::yield_now().await;
  let cancelled = spawn(1);
  tokio::task::yield_now().await;
  let survivor = spawn(2);
  tokio::task::yield_now().await;
  cancelled.abort();
  owner.abort();
  assert!(cancelled.await.unwrap_err().is_cancelled());
  assert!(owner.await.unwrap_err().is_cancelled());
  let newcomer = spawn(3);
  survivor.await.unwrap();
  newcomer.await.unwrap();
  assert_eq!(*order.lock().unwrap(), vec![2, 3]);
  assert!(start.elapsed() >= Duration::from_secs(2));
  assert!(start.elapsed() < Duration::from_secs(3));
}

#[tokio::test(start_paused = true)]
async fn idle_and_network_blocked_connections_do_not_hold_bandwidth_turns() {
  let rate = Arc::new(UploadRate::new(1041));
  let idle_connection = rate.clone();
  let (ready, started) = tokio::sync::oneshot::channel();
  let slow_rate = rate.clone();
  let slow = tokio::spawn(async move {
    slow_rate.acquire(1041).await;
    ready.send(()).unwrap();
    // The sender can be blocked in network I/O after receiving its grant.
    std::future::pending::<()>().await;
  });
  started.await.unwrap();
  let start = Instant::now();
  for _ in 0..3 {
    rate.acquire(1041).await;
  }
  assert!(start.elapsed() >= Duration::from_secs(3));
  assert!(start.elapsed() < Duration::from_secs(4));
  assert!(!slow.is_finished());
  slow.abort();
  assert!(slow.await.unwrap_err().is_cancelled());
  drop(idle_connection);
}
