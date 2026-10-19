//! Exclusive managed admission, or concurrent caller-managed accept operations.
use crate::control::ControlError;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(crate) struct Admission(Mutex<Owners>);
#[derive(Default)]
struct Owners {
  managed: bool,
  manual: usize,
}
#[derive(Clone, Copy)]
enum Mode {
  Managed,
  Manual,
}
pub(crate) struct AdmissionLease {
  admission: Arc<Admission>,
  mode: Mode,
}
impl Admission {
  pub(crate) fn managed(self: &Arc<Self>) -> Result<AdmissionLease, ControlError> {
    self.claim(Mode::Managed)
  }
  pub(crate) fn manual(self: &Arc<Self>) -> Result<AdmissionLease, ControlError> {
    self.claim(Mode::Manual)
  }
  fn claim(self: &Arc<Self>, mode: Mode) -> Result<AdmissionLease, ControlError> {
    let mut owners = self.0.lock().unwrap_or_else(|e| e.into_inner());
    if owners.managed || (matches!(mode, Mode::Managed) && owners.manual != 0) {
      return Err(ControlError::AdmissionConflict);
    }
    match mode {
      Mode::Managed => owners.managed = true,
      Mode::Manual => owners.manual += 1,
    }
    Ok(AdmissionLease {
      admission: self.clone(),
      mode,
    })
  }
}
impl Drop for AdmissionLease {
  fn drop(&mut self) {
    let mut owners = self.admission.0.lock().unwrap_or_else(|e| e.into_inner());
    match self.mode {
      Mode::Managed => owners.managed = false,
      Mode::Manual => owners.manual -= 1,
    }
  }
}

#[cfg(test)]
#[path = "../tests/admission.rs"]
mod tests;
