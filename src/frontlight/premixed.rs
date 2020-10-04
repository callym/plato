use super::{Frontlight, LightLevels};
use crate::device::{Model, CURRENT_DEVICE};
use anyhow::Error;
use std::{
  fs::{File, OpenOptions},
  io::Write,
  path::PathBuf,
};

const FRONTLIGHT_INTERFACE: &str = "/sys/class/backlight";
const FRONTLIGHT_WHITE: &str = "mxc_msp430.0/brightness";
// Forma
const FRONTLIGHT_ORANGE_A: &str = "tlc5947_bl/color";
// Libra H₂O, Clara HD
const FRONTLIGHT_ORANGE_B: &str = "lm3630a_led/color";

pub struct PremixedFrontlight {
  intensity: f32,
  warmth: f32,
  white: File,
  orange: File,
}

impl PremixedFrontlight {
  pub fn new(intensity: f32, warmth: f32) -> Result<PremixedFrontlight, Error> {
    let base = PathBuf::from(FRONTLIGHT_INTERFACE);
    let white = OpenOptions::new()
      .write(true)
      .open(base.join(FRONTLIGHT_WHITE))?;
    let model = CURRENT_DEVICE.model;
    let orange_path = base.join(if model == Model::Forma || model == Model::Forma32GB {
      FRONTLIGHT_ORANGE_A
    } else {
      FRONTLIGHT_ORANGE_B
    });
    let orange = OpenOptions::new().write(true).open(orange_path)?;
    Ok(PremixedFrontlight {
      intensity,
      warmth,
      white,
      orange,
    })
  }
}

impl Frontlight for PremixedFrontlight {
  fn set_intensity(&mut self, intensity: f32) {
    let white = intensity.round() as i16;
    write!(self.white, "{}", white).unwrap();
    self.intensity = intensity;
  }

  fn set_warmth(&mut self, warmth: f32) {
    let orange = 10 - (warmth / 10.0).round() as i16;
    write!(self.orange, "{}", orange).unwrap();
    self.warmth = warmth;
  }

  fn levels(&self) -> LightLevels {
    LightLevels {
      intensity: self.intensity,
      warmth: self.warmth,
    }
  }
}
