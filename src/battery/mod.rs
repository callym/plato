mod fake;
mod kobo;

use anyhow::Error;

pub use self::{fake::FakeBattery, kobo::KoboBattery};

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Status {
  Discharging,
  Charging,
  Charged,
  // Full,
  // Unknown
}

pub trait Battery {
  fn capacity(&mut self) -> Result<f32, Error>;
  fn status(&mut self) -> Result<Status, Error>;
}
