use super::{Bus, Event, Hub, Id, RenderData, RenderQueue, View, ViewId, ID_FEEDER};
use crate::{
  app::Context,
  color::{BLACK, WHITE},
  device::CURRENT_DEVICE,
  font::{font_from_style, Fonts, NORMAL_STYLE},
  framebuffer::{Framebuffer, UpdateMode},
  geom::Rectangle,
  gesture::GestureEvent,
};
use chrono::{DateTime, Local};

pub struct Clock {
  id: Id,
  rect: Rectangle,
  children: Vec<Box<dyn View>>,
  time: DateTime<Local>,
}

impl Clock {
  pub fn new(rect: &mut Rectangle, fonts: &mut Fonts) -> Clock {
    let font = font_from_style(fonts, &NORMAL_STYLE, CURRENT_DEVICE.dpi);
    let width = font.plan("00:00", None, None).width + font.em() as i32;
    rect.min.x = rect.max.x - width;
    Clock {
      id: ID_FEEDER.next(),
      rect: *rect,
      children: vec![],
      time: Local::now(),
    }
  }
}

impl View for Clock {
  fn handle_event(
    &mut self,
    evt: &Event,
    _hub: &Hub,
    bus: &mut Bus,
    rq: &mut RenderQueue,
    _context: &mut Context,
  ) -> bool {
    match *evt {
      Event::ClockTick => {
        self.time = Local::now();
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
        true
      },
      Event::Gesture(GestureEvent::Tap(center)) if self.rect.includes(center) => {
        bus.push_back(Event::ToggleNear(ViewId::ClockMenu, self.rect));
        true
      },
      _ => false,
    }
  }

  fn render(&self, fb: &mut dyn Framebuffer, _rect: Rectangle, fonts: &mut Fonts) {
    let dpi = CURRENT_DEVICE.dpi;
    let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
    let plan = font.plan(&self.time.format("%H:%M").to_string(), None, None);
    let dx = (self.rect.width() as i32 - plan.width) / 2;
    let dy = (self.rect.height() as i32 - font.x_heights.0 as i32) / 2;
    let pt = pt!(self.rect.min.x + dx, self.rect.max.y - dy);

    fb.draw_rectangle(&self.rect, WHITE);
    font.render(fb, BLACK, &plan, pt);
  }

  fn rect(&self) -> &Rectangle {
    &self.rect
  }

  fn rect_mut(&mut self) -> &mut Rectangle {
    &mut self.rect
  }

  fn children(&self) -> &Vec<Box<dyn View>> {
    &self.children
  }

  fn children_mut(&mut self) -> &mut Vec<Box<dyn View>> {
    &mut self.children
  }

  fn id(&self) -> Id {
    self.id
  }
}
