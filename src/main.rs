#[macro_use]
mod geom;
mod app;
mod battery;
mod color;
mod device;
mod dictionary;
mod document;
mod font;
mod framebuffer;
mod frontlight;
mod gesture;
mod helpers;
mod input;
mod library;
mod lightsensor;
mod metadata;
mod rtc;
mod settings;
mod symbolic_path;
mod unit;
mod view;

use crate::app::run;
use anyhow::Error;

fn main() -> Result<(), Error> {
  run()?;
  Ok(())
}
