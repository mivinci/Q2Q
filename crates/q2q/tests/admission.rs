use super::*;

#[test]
fn concurrent_manual_accepts_exclude_managed_publication_until_both_finish() {
  let admission = Arc::new(Admission::default());
  let first = admission.manual().unwrap();
  let second = admission.manual().unwrap();
  assert!(matches!(
    admission.managed(),
    Err(ControlError::AdmissionConflict)
  ));
  drop(first);
  assert!(matches!(
    admission.managed(),
    Err(ControlError::AdmissionConflict)
  ));
  drop(second);
  let managed = admission.managed().unwrap();
  assert!(matches!(
    admission.manual(),
    Err(ControlError::AdmissionConflict)
  ));
  assert!(matches!(
    admission.managed(),
    Err(ControlError::AdmissionConflict)
  ));
  drop(managed);
  assert!(admission.manual().is_ok());
}

#[tokio::test]
async fn cancellation_before_first_poll_releases_admission_ownership() {
  let admission = Arc::new(Admission::default());
  let lease = admission.managed().unwrap();
  let task = tokio::spawn(async move {
    let _lease = lease;
    std::future::pending::<()>().await;
  });
  task.abort();
  assert!(task.await.unwrap_err().is_cancelled());
  assert!(admission.manual().is_ok());
}
