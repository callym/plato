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

use crate::{
  app::Context,
  battery::{Battery, FakeBattery},
  device::CURRENT_DEVICE,
  document::sys_info_as_html,
  font::Fonts,
  framebuffer::{Framebuffer, UpdateMode},
  frontlight::{Frontlight, LightLevels},
  geom::Rectangle,
  gesture::gesture_events,
  helpers::{load_toml, save_toml},
  input::{DeviceEvent, FingerStatus},
  library::Library,
  lightsensor::LightSensor,
  settings::{Settings, SETTINGS_PATH},
  view::{
    calculator::Calculator,
    common::{
      locate,
      locate_by_id,
      overlapping_rectangle,
      toggle_input_history_menu,
      toggle_keyboard_layout_menu,
      transfer_notifications,
    },
    dialog::Dialog,
    dictionary::Dictionary,
    frontlight::FrontlightWindow,
    handle_event,
    home::Home,
    menu::{Menu, MenuKind},
    notification::Notification,
    process_render_queue,
    reader::Reader,
    sketch::Sketch,
    AppCmd,
    EntryId,
    EntryKind,
    Event,
    RenderData,
    RenderQueue,
    View,
    ViewId,
  },
};
use anyhow::{Context as ResultExt, Error};
use chrono::Local;
use fxhash::FxHashMap;
use sdl2::{
  event::Event as SdlEvent,
  keyboard::{Keycode, Scancode},
  pixels::{Color as SdlColor, PixelFormatEnum},
  rect::{Point as SdlPoint, Rect as SdlRect},
  render::{BlendMode, WindowCanvas},
};
use std::{collections::VecDeque, fs::File, mem, path::Path, sync::mpsc, thread, time::Duration};

pub const APP_NAME: &str = "Plato";
const DEFAULT_ROTATION: i8 = 1;

const CLOCK_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

pub fn build_context(fb: Box<dyn Framebuffer>) -> Result<Context, Error> {
  let settings = load_toml::<Settings, _>(SETTINGS_PATH)?;
  let library_settings = &settings.libraries[settings.selected_library];
  let library = Library::new(&library_settings.path, library_settings.mode);

  let battery = Box::new(FakeBattery::new()) as Box<dyn Battery>;
  let frontlight = Box::new(LightLevels::default()) as Box<dyn Frontlight>;
  let lightsensor = Box::new(0u16) as Box<dyn LightSensor>;
  let fonts = Fonts::load()?;

  Ok(Context::new(
    fb,
    None,
    library,
    settings,
    fonts,
    battery,
    frontlight,
    lightsensor,
  ))
}

#[inline]
fn seconds(timestamp: u32) -> f64 {
  timestamp as f64 / 1000.0
}

#[inline]
pub fn device_event(event: SdlEvent) -> Option<DeviceEvent> {
  match event {
    SdlEvent::MouseButtonDown {
      timestamp, x, y, ..
    } => Some(DeviceEvent::Finger {
      id: 0,
      status: FingerStatus::Down,
      position: pt!(x, y),
      time: seconds(timestamp),
    }),
    SdlEvent::MouseButtonUp {
      timestamp, x, y, ..
    } => Some(DeviceEvent::Finger {
      id: 0,
      status: FingerStatus::Up,
      position: pt!(x, y),
      time: seconds(timestamp),
    }),
    SdlEvent::MouseMotion {
      timestamp, x, y, ..
    } => Some(DeviceEvent::Finger {
      id: 0,
      status: FingerStatus::Motion,
      position: pt!(x, y),
      time: seconds(timestamp),
    }),
    _ => None,
  }
}

impl Framebuffer for WindowCanvas {
  fn set_pixel(&mut self, x: u32, y: u32, color: u8) {
    self.set_draw_color(SdlColor::RGB(color, color, color));
    self.draw_point(SdlPoint::new(x as i32, y as i32)).unwrap();
  }

  fn set_blended_pixel(&mut self, x: u32, y: u32, color: u8, alpha: f32) {
    self.set_draw_color(SdlColor::RGBA(color, color, color, (alpha * 255.0) as u8));
    self.draw_point(SdlPoint::new(x as i32, y as i32)).unwrap();
  }

  fn invert_region(&mut self, rect: &Rectangle) {
    let width = rect.width();
    let s_rect = Some(SdlRect::new(rect.min.x, rect.min.y, width, rect.height()));
    if let Ok(data) = self.read_pixels(s_rect, PixelFormatEnum::RGB24) {
      for y in rect.min.y..rect.max.y {
        let v = (y - rect.min.y) as u32;
        for x in rect.min.x..rect.max.x {
          let u = (x - rect.min.x) as u32;
          let addr = 3 * (v * width + u);
          let color = 255 - data[addr as usize];
          self.set_pixel(x as u32, y as u32, color);
        }
      }
    }
  }

  fn shift_region(&mut self, rect: &Rectangle, drift: u8) {
    let width = rect.width();
    let s_rect = Some(SdlRect::new(rect.min.x, rect.min.y, width, rect.height()));
    if let Ok(data) = self.read_pixels(s_rect, PixelFormatEnum::RGB24) {
      for y in rect.min.y..rect.max.y {
        let v = (y - rect.min.y) as u32;
        for x in rect.min.x..rect.max.x {
          let u = (x - rect.min.x) as u32;
          let addr = 3 * (v * width + u);
          let color = data[addr as usize].saturating_sub(drift);
          self.set_pixel(x as u32, y as u32, color);
        }
      }
    }
  }

  fn update(&mut self, _rect: &Rectangle, _mode: UpdateMode) -> Result<u32, Error> {
    self.present();
    Ok(Local::now().timestamp_subsec_millis())
  }

  fn wait(&self, _tok: u32) -> Result<i32, Error> {
    Ok(1)
  }

  fn save(&self, path: &str) -> Result<(), Error> {
    let (width, height) = self.dims();
    let file = File::create(path).with_context(|| format!("Can't create output file {}.", path))?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_color(png::ColorType::RGB);
    let mut writer = encoder
      .write_header()
      .with_context(|| format!("Can't write PNG header for {}.", path))?;
    let data = self
      .read_pixels(self.viewport(), PixelFormatEnum::RGB24)
      .unwrap_or_default();
    writer
      .write_image_data(&data)
      .with_context(|| format!("Can't write PNG data to {}.", path))?;
    Ok(())
  }

  fn rotation(&self) -> i8 {
    DEFAULT_ROTATION
  }

  fn set_rotation(&mut self, n: i8) -> Result<(u32, u32), Error> {
    let (mut width, mut height) = self.dims();
    if (width < height && n % 2 == 0) || (width > height && n % 2 == 1) {
      mem::swap(&mut width, &mut height);
    }
    self.window_mut().set_size(width, height).ok();
    Ok((width, height))
  }

  fn set_inverted(&mut self, _enable: bool) {}

  fn set_monochrome(&mut self, _enable: bool) {}

  fn inverted(&self) -> bool {
    false
  }

  fn monochrome(&self) -> bool {
    false
  }

  fn dims(&self) -> (u32, u32) {
    self.window().size()
  }
}

fn main() -> Result<(), Error> {
  let sdl_context = sdl2::init().unwrap();
  let video_subsystem = sdl_context.video().unwrap();
  let (width, height) = CURRENT_DEVICE.dims;
  let window = video_subsystem
    .window("Plato Emulator", width, height)
    .position_centered()
    .build()
    .unwrap();

  let mut fb = window.into_canvas().software().build().unwrap();
  fb.set_blend_mode(BlendMode::Blend);

  let mut context = build_context(Box::new(fb))?;

  if context.settings.import.startup_trigger {
    context.batch_import();
  }

  context.load_dictionaries();
  context.load_keyboard_layouts();

  let (tx, rx) = mpsc::channel();
  let (ty, ry) = mpsc::channel();
  let touch_screen = gesture_events(ry);

  let tx2 = tx.clone();
  thread::spawn(move || {
    while let Ok(evt) = touch_screen.recv() {
      tx2.send(evt).ok();
    }
  });

  let tx3 = tx.clone();
  thread::spawn(move || loop {
    thread::sleep(CLOCK_REFRESH_INTERVAL);
    tx3.send(Event::ClockTick).ok();
  });

  let mut history: Vec<Box<dyn View>> = Vec::new();
  let mut rq = RenderQueue::new();
  let mut view: Box<dyn View> = Box::new(Home::new(context.fb.rect(), &mut rq, &mut context)?);

  let mut updating = FxHashMap::default();

  if context.settings.frontlight {
    let levels = context.settings.frontlight_levels;
    context.frontlight.set_intensity(levels.intensity);
    context.frontlight.set_warmth(levels.warmth);
  } else {
    context.frontlight.set_warmth(0.0);
    context.frontlight.set_intensity(0.0);
  }

  println!(
    "{} is running on a Kobo {}.",
    APP_NAME, CURRENT_DEVICE.model
  );
  println!(
    "The framebuffer resolution is {} by {}.",
    context.fb.rect().width(),
    context.fb.rect().height()
  );

  let mut bus = VecDeque::with_capacity(4);

  'outer: loop {
    if let Some(sdl_evt) = sdl_context.event_pump().unwrap().wait_event_timeout(20) {
      match sdl_evt {
        SdlEvent::Quit { .. }
        | SdlEvent::KeyDown {
          keycode: Some(Keycode::Escape),
          ..
        } => {
          view.handle_event(
            &Event::Back,
            &tx,
            &mut VecDeque::new(),
            &mut RenderQueue::new(),
            &mut context,
          );
          while let Some(mut view) = history.pop() {
            view.handle_event(
              &Event::Back,
              &tx,
              &mut VecDeque::new(),
              &mut RenderQueue::new(),
              &mut context,
            );
          }
          break;
        },
        SdlEvent::KeyDown {
          scancode: Some(scancode),
          ..
        } => match scancode {
          Scancode::LeftBracket => {
            let rot = (3 + context.display.rotation) % 4;
            ty.send(DeviceEvent::RotateScreen(rot)).ok();
          },
          Scancode::RightBracket => {
            let rot = (5 + context.display.rotation) % 4;
            ty.send(DeviceEvent::RotateScreen(rot)).ok();
          },
          _ => (),
        },
        _ => {
          if let Some(dev_evt) = device_event(sdl_evt) {
            ty.send(dev_evt).ok();
          }
        },
      }
    }

    while let Ok(evt) = rx.recv_timeout(Duration::from_millis(20)) {
      match evt {
        Event::Open(info) => {
          let rotation = context.display.rotation;
          if let Some(n) = info
            .reader
            .as_ref()
            .and_then(|r| r.rotation.map(|n| CURRENT_DEVICE.from_canonical(n)))
          {
            if n != rotation {
              if let Ok(dims) = context.fb.set_rotation(n) {
                context.display.rotation = n;
                context.display.dims = dims;
              }
            }
          }
          let info2 = info.clone();
          if let Some(r) = Reader::new(context.fb.rect(), *info, &tx, &mut context) {
            let mut next_view = Box::new(r) as Box<dyn View>;
            transfer_notifications(view.as_mut(), next_view.as_mut(), &mut rq, &mut context);
            history.push(view as Box<dyn View>);
            view = next_view;
          } else {
            if context.display.rotation != rotation {
              if let Ok(dims) = context.fb.set_rotation(rotation) {
                context.display.rotation = rotation;
                context.display.dims = dims;
              }
            }
            handle_event(
              view.as_mut(),
              &Event::Invalid(info2),
              &tx,
              &mut bus,
              &mut rq,
              &mut context,
            );
          }
        },
        Event::OpenToc(ref toc, chap_index) => {
          let r = Reader::from_toc(context.fb.rect(), toc, chap_index, &tx, &mut context);
          let mut next_view = Box::new(r) as Box<dyn View>;
          transfer_notifications(view.as_mut(), next_view.as_mut(), &mut rq, &mut context);
          history.push(view as Box<dyn View>);
          view = next_view;
        },
        Event::Select(EntryId::Launch(app_cmd)) => {
          view.children_mut().retain(|child| !child.is::<Menu>());
          let mut next_view: Box<dyn View> = match app_cmd {
            AppCmd::Sketch => Box::new(Sketch::new(context.fb.rect(), &mut rq, &mut context)),
            AppCmd::Calculator => Box::new(Calculator::new(
              context.fb.rect(),
              &tx,
              &mut rq,
              &mut context,
            )?),
            AppCmd::Dictionary {
              ref query,
              ref language,
            } => Box::new(Dictionary::new(
              context.fb.rect(),
              query,
              language,
              &tx,
              &mut rq,
              &mut context,
            )),
          };
          transfer_notifications(view.as_mut(), next_view.as_mut(), &mut rq, &mut context);
          history.push(view as Box<dyn View>);
          view = next_view;
        },
        Event::Back => {
          if let Some(v) = history.pop() {
            view = v;
            if view.is::<Home>() {
              if context.display.rotation % 2 != 1 {
                if let Ok(dims) = context.fb.set_rotation(DEFAULT_ROTATION) {
                  context.display.rotation = DEFAULT_ROTATION;
                  context.display.dims = dims;
                }
              }
            }
            view.handle_event(&Event::Reseed, &tx, &mut bus, &mut rq, &mut context);
          }
        },
        Event::TogglePresetMenu(rect, index) => {
          if let Some(index) = locate_by_id(view.as_ref(), ViewId::PresetMenu) {
            let rect = *view.child(index).rect();
            view.children_mut().remove(index);
            rq.add(RenderData::expose(rect, UpdateMode::Gui));
          } else {
            let preset_menu = Menu::new(
              rect,
              ViewId::PresetMenu,
              MenuKind::Contextual,
              vec![EntryKind::Command(
                "Remove".to_string(),
                EntryId::RemovePreset(index),
              )],
              &mut context,
            );
            rq.add(RenderData::new(
              preset_menu.id(),
              *preset_menu.rect(),
              UpdateMode::Gui,
            ));
            view
              .children_mut()
              .push(Box::new(preset_menu) as Box<dyn View>);
          }
        },
        Event::Show(ViewId::Frontlight) => {
          if !context.settings.frontlight {
            context.set_frontlight(true);
            view.handle_event(
              &Event::ToggleFrontlight,
              &tx,
              &mut bus,
              &mut rq,
              &mut context,
            );
          }
          let flw = FrontlightWindow::new(&mut context);
          rq.add(RenderData::new(flw.id(), *flw.rect(), UpdateMode::Gui));
          view.children_mut().push(Box::new(flw) as Box<dyn View>);
        },
        Event::ToggleFrontlight => {
          context.set_frontlight(!context.settings.frontlight);
          view.handle_event(
            &Event::ToggleFrontlight,
            &tx,
            &mut bus,
            &mut rq,
            &mut context,
          );
        },
        Event::ToggleInputHistoryMenu(id, rect) => {
          toggle_input_history_menu(view.as_mut(), id, rect, None, &mut rq, &mut context);
        },
        Event::ToggleNear(ViewId::KeyboardLayoutMenu, rect) => {
          toggle_keyboard_layout_menu(view.as_mut(), rect, None, &mut rq, &mut context);
        },
        Event::Close(ViewId::Frontlight) => {
          if let Some(index) = locate::<FrontlightWindow>(view.as_ref()) {
            let rect = *view.child(index).rect();
            view.children_mut().remove(index);
            rq.add(RenderData::expose(rect, UpdateMode::Gui));
          }
        },
        Event::Close(id) => {
          if let Some(index) = locate_by_id(view.as_ref(), id) {
            let rect = overlapping_rectangle(view.child(index));
            rq.add(RenderData::expose(rect, UpdateMode::Gui));
            view.children_mut().remove(index);
          }
        },
        Event::Select(EntryId::About) => {
          let dialog = Dialog::new(
            ViewId::AboutDialog,
            None,
            format!("Plato {}", env!("CARGO_PKG_VERSION")),
            &mut context,
          );
          rq.add(RenderData::new(
            dialog.id(),
            *dialog.rect(),
            UpdateMode::Gui,
          ));
          view.children_mut().push(Box::new(dialog) as Box<dyn View>);
        },
        Event::Select(EntryId::SystemInfo) => {
          view.children_mut().retain(|child| !child.is::<Menu>());
          let html = sys_info_as_html();
          let r = Reader::from_html(context.fb.rect(), &html, &tx, &mut context);
          let mut next_view = Box::new(r) as Box<dyn View>;
          transfer_notifications(view.as_mut(), next_view.as_mut(), &mut rq, &mut context);
          history.push(view as Box<dyn View>);
          view = next_view;
        },
        Event::Select(EntryId::Rotate(n))
          if n != context.display.rotation && view.might_rotate() =>
        {
          updating.retain(|tok, _| context.fb.wait(*tok).is_err());
          if let Ok(dims) = context.fb.set_rotation(n) {
            context.display.rotation = n;
            let fb_rect = Rectangle::from(dims);
            if context.display.dims != dims {
              context.display.dims = dims;
              view.resize(fb_rect, &tx, &mut rq, &mut context);
            }
          }
        }
        Event::Select(EntryId::SetButtonScheme(button_scheme)) => {
          context.settings.button_scheme = button_scheme;
        },
        Event::Select(EntryId::ToggleInverted) => {
          context.fb.toggle_inverted();
          rq.add(RenderData::new(
            view.id(),
            context.fb.rect(),
            UpdateMode::Gui,
          ));
        },
        Event::Select(EntryId::ToggleMonochrome) => {
          context.fb.toggle_monochrome();
          rq.add(RenderData::new(
            view.id(),
            context.fb.rect(),
            UpdateMode::Gui,
          ));
        },
        Event::Select(EntryId::TakeScreenshot) => {
          let name = Local::now().format("screenshot-%Y%m%d_%H%M%S.png");
          let msg = match context.fb.save(&name.to_string()) {
            Err(e) => format!("Couldn't take screenshot: {}).", e),
            Ok(_) => format!("Saved {}.", name),
          };
          let notif =
            Notification::new(ViewId::TakeScreenshotNotif, msg, &tx, &mut rq, &mut context);
          view.children_mut().push(Box::new(notif) as Box<dyn View>);
        },
        Event::Notify(msg) => {
          let notif = Notification::new(ViewId::MessageNotif, msg, &tx, &mut rq, &mut context);
          view.children_mut().push(Box::new(notif) as Box<dyn View>);
        },
        Event::AddDocument(..) => {
          if view.is::<Home>() {
            view.handle_event(&evt, &tx, &mut bus, &mut rq, &mut context);
          } else {
            let (tx, _rx) = mpsc::channel();
            history[0].handle_event(
              &evt,
              &tx,
              &mut VecDeque::new(),
              &mut RenderQueue::new(),
              &mut context,
            );
          };
        },
        Event::SetWifi(enable) => {
          if context.settings.wifi != enable {
            context.settings.wifi = enable;
            if enable {
              let tx2 = tx.clone();
              thread::spawn(move || {
                thread::sleep(Duration::from_secs(2));
                tx2.send(Event::Device(DeviceEvent::NetUp)).ok();
              });
            } else {
              context.online = false;
            }
          }
        },
        Event::Device(DeviceEvent::NetUp) => {
          if view.is::<Home>() {
            view.handle_event(&evt, &tx, &mut bus, &mut rq, &mut context);
          } else {
            let (tx, _rx) = mpsc::channel();
            history[0].handle_event(
              &evt,
              &tx,
              &mut VecDeque::new(),
              &mut RenderQueue::new(),
              &mut context,
            );
          };
        },
        Event::Device(DeviceEvent::RotateScreen(n)) => {
          tx.send(Event::Select(EntryId::Rotate(n))).ok();
        },
        Event::Select(EntryId::Quit) => {
          break 'outer;
        },
        _ => {
          handle_event(view.as_mut(), &evt, &tx, &mut bus, &mut rq, &mut context);
        },
      }
    }

    process_render_queue(view.as_ref(), &mut rq, &mut context, &mut updating);

    while let Some(ce) = bus.pop_front() {
      tx.send(ce).ok();
    }
  }

  if !history.is_empty() {
    let (tx, _rx) = mpsc::channel();
    view.handle_event(
      &Event::Back,
      &tx,
      &mut VecDeque::new(),
      &mut RenderQueue::new(),
      &mut context,
    );
    while let Some(mut view) = history.pop() {
      view.handle_event(
        &Event::Back,
        &tx,
        &mut VecDeque::new(),
        &mut RenderQueue::new(),
        &mut context,
      );
    }
  }

  if context.settings.frontlight {
    context.settings.frontlight_levels = context.frontlight.levels();
  }

  context.library.flush();

  let path = Path::new(SETTINGS_PATH);
  save_toml(&context.settings, path).context("Can't save settings.")?;

  Ok(())
}
