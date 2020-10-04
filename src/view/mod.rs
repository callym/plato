//! Views are organized as a tree. A view might receive / send events and render itself.
//!
//! The z-level of the n-th child of a view is less or equal to the z-level of its n+1-th child.
//!
//! Events travel from the root to the leaves, only the leaf views will handle the root events, but
//! any view can send events to its parent. From the events it receives from its children, a view
//! resends the ones it doesn't handle to its own parent. Hence an event sent from a child might
//! bubble up to the root. If it reaches the root without being captured by any view, then it will
//! be written to the main event channel and will be sent to every leaf in one of the next loop
//! iterations.

pub mod battery;
pub mod button;
pub mod calculator;
pub mod clock;
pub mod common;
pub mod dialog;
pub mod dictionary;
pub mod filler;
pub mod frontlight;
pub mod home;
pub mod icon;
pub mod image;
pub mod input_field;
pub mod intermission;
pub mod key;
pub mod keyboard;
pub mod label;
pub mod labeled_icon;
pub mod menu;
pub mod menu_entry;
pub mod named_input;
pub mod notification;
pub mod page_label;
pub mod preset;
pub mod presets_list;
pub mod reader;
pub mod rounded_button;
pub mod search_bar;
pub mod sketch;
pub mod slider;
pub mod top_bar;

use self::{calculator::LineOrigin, intermission::IntermKind, key::KeyKind};
use crate::{
  app::Context,
  document::{Location, TextLocation, TocEntry},
  font::Fonts,
  framebuffer::{Framebuffer, UpdateMode},
  geom::{Boundary, CycleDir, LinearDir, Rectangle},
  gesture::GestureEvent,
  input::{DeviceEvent, FingerStatus},
  metadata::{Info, Margin, PageScheme, SimpleStatus, SortMethod, TextAlign, ZoomMode},
  settings::{ButtonScheme, FirstColumn, RotationLock, SecondColumn},
};
use downcast_rs::{impl_downcast, Downcast};
use fxhash::FxHashMap;
use std::{
  collections::VecDeque,
  fmt::{self, Debug},
  ops::{Deref, DerefMut},
  path::PathBuf,
  sync::{
    atomic::{AtomicU64, Ordering},
    mpsc::Sender,
  },
  time::Duration,
};

// Border thicknesses in pixels, at 300 DPI.
pub const THICKNESS_SMALL: f32 = 1.0;
pub const THICKNESS_MEDIUM: f32 = 2.0;
pub const THICKNESS_LARGE: f32 = 3.0;

// Border radii in pixels, at 300 DPI.
pub const BORDER_RADIUS_SMALL: f32 = 6.0;
pub const BORDER_RADIUS_MEDIUM: f32 = 9.0;
pub const BORDER_RADIUS_LARGE: f32 = 12.0;

// Big and small bar heights in pixels, at 300 DPI.
// On the *Aura ONE*, the height is exactly `2 * sb + 10 * bb`.
pub const SMALL_BAR_HEIGHT: f32 = 121.0;
pub const BIG_BAR_HEIGHT: f32 = 163.0;

pub const CLOSE_IGNITION_DELAY: Duration = Duration::from_millis(150);

pub type Bus = VecDeque<Event>;
pub type Hub = Sender<Event>;

pub trait View: Downcast {
  fn handle_event(
    &mut self,
    evt: &Event,
    hub: &Hub,
    bus: &mut Bus,
    rq: &mut RenderQueue,
    context: &mut Context,
  ) -> bool;
  fn render(&self, fb: &mut dyn Framebuffer, rect: Rectangle, fonts: &mut Fonts);
  fn rect(&self) -> &Rectangle;
  fn rect_mut(&mut self) -> &mut Rectangle;
  fn children(&self) -> &Vec<Box<dyn View>>;
  fn children_mut(&mut self) -> &mut Vec<Box<dyn View>>;
  fn id(&self) -> Id;

  fn render_rect(&self, _rect: &Rectangle) -> Rectangle {
    *self.rect()
  }

  fn resize(&mut self, rect: Rectangle, _hub: &Hub, _rq: &mut RenderQueue, _context: &mut Context) {
    *self.rect_mut() = rect;
  }

  fn child(&self, index: usize) -> &dyn View {
    self.children()[index].as_ref()
  }

  fn child_mut(&mut self, index: usize) -> &mut dyn View {
    self.children_mut()[index].as_mut()
  }

  fn len(&self) -> usize {
    self.children().len()
  }

  fn might_skip(&self, _evt: &Event) -> bool {
    false
  }

  fn might_rotate(&self) -> bool {
    true
  }

  fn is_background(&self) -> bool {
    false
  }

  fn view_id(&self) -> Option<ViewId> {
    None
  }
}

impl_downcast!(View);

impl Debug for Box<dyn View> {
  fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
    write!(f, "Box<dyn View>")
  }
}

// We start delivering events from the highest z-level to prevent views from capturing
// gestures that occurred in higher views.
// The consistency must also be ensured by the views: popups, for example, need to
// capture any tap gesture with a touch point inside their rectangle.
// A child can send events to the main channel through the *hub* or communicate with its parent through the *bus*.
// A view that wants to render can write to the rendering queue.
pub fn handle_event(
  view: &mut dyn View,
  evt: &Event,
  hub: &Hub,
  parent_bus: &mut Bus,
  rq: &mut RenderQueue,
  context: &mut Context,
) -> bool {
  if view.len() > 0 {
    let mut captured = false;

    if view.might_skip(evt) {
      return captured;
    }

    let mut child_bus: Bus = VecDeque::with_capacity(1);

    for i in (0..view.len()).rev() {
      if handle_event(view.child_mut(i), evt, hub, &mut child_bus, rq, context) {
        captured = true;
        break;
      }
    }

    let mut temp_bus: Bus = VecDeque::with_capacity(1);

    child_bus.retain(|child_evt| !view.handle_event(child_evt, hub, &mut temp_bus, rq, context));

    parent_bus.append(&mut child_bus);
    parent_bus.append(&mut temp_bus);

    captured || view.handle_event(evt, hub, parent_bus, rq, context)
  } else {
    view.handle_event(evt, hub, parent_bus, rq, context)
  }
}

// We render from bottom to top. For a view to render it has to either appear in `ids` or intersect
// one of the rectangles in `bgs`. When we're about to render a view, if `wait` is true, we'll wait
// for all the updates in `updating` that intersect with the view.
pub fn render(
  view: &dyn View,
  wait: bool,
  ids: &FxHashMap<Id, Vec<Rectangle>>,
  rects: &mut Vec<Rectangle>,
  bgs: &mut Vec<Rectangle>,
  fb: &mut dyn Framebuffer,
  fonts: &mut Fonts,
  updating: &mut FxHashMap<u32, Rectangle>,
) {
  let mut render_rects = Vec::new();

  if view.len() == 0 || view.is_background() {
    for rect in ids
      .get(&view.id())
      .cloned()
      .into_iter()
      .flatten()
      .chain(rects.iter().filter_map(|r| r.intersection(view.rect())))
      .chain(bgs.iter().filter_map(|r| r.intersection(view.rect())))
    {
      let render_rect = view.render_rect(&rect);

      if wait {
        updating.retain(|tok, urect| {
          !render_rect.overlaps(urect) || fb.wait(*tok).map_err(|err| eprintln!("{}", err)).is_err()
        });
      }

      view.render(fb, rect, fonts);
      render_rects.push(render_rect);

      // Most views can't render a subrectangle of themselves.
      if *view.rect() == render_rect {
        break;
      }
    }
  } else {
    bgs.extend(ids.get(&view.id()).cloned().into_iter().flatten());
  }

  // Merge the contiguous zones to avoid having to schedule lots of small frambuffer updates.
  for rect in render_rects.into_iter() {
    if rects.is_empty() {
      rects.push(rect);
    } else {
      if let Some(last) = rects.last_mut() {
        if rect.touches(last) {
          last.absorb(&rect);
          let mut i = rects.len();
          while i > 1 && rects[i - 1].touches(&rects[i - 2]) {
            if let Some(rect) = rects.pop() {
              if let Some(last) = rects.last_mut() {
                last.absorb(&rect);
              }
            }
            i -= 1;
          }
        } else {
          let mut i = rects.len();
          while i > 0 && !rects[i - 1].contains(&rect) {
            i -= 1;
          }
          if i == 0 {
            rects.push(rect);
          }
        }
      }
    }
  }

  for i in 0..view.len() {
    render(view.child(i), wait, ids, rects, bgs, fb, fonts, updating);
  }
}

#[inline]
pub fn process_render_queue(
  view: &dyn View,
  rq: &mut RenderQueue,
  context: &mut Context,
  updating: &mut FxHashMap<u32, Rectangle>,
) {
  for ((mode, wait), pairs) in rq.drain() {
    let mut ids = FxHashMap::default();
    let mut rects = Vec::new();
    let mut bgs = Vec::new();

    for (id, rect) in pairs.into_iter().rev() {
      if let Some(id) = id {
        ids.entry(id).or_insert_with(|| Vec::new()).push(rect);
      } else {
        bgs.push(rect);
      }
    }

    render(
      view,
      wait,
      &ids,
      &mut rects,
      &mut bgs,
      context.fb.as_mut(),
      &mut context.fonts,
      updating,
    );

    for rect in rects {
      match context.fb.update(&rect, mode) {
        Ok(tok) => {
          updating.insert(tok, rect);
        },
        Err(err) => {
          eprintln!("{}", err);
        },
      }
    }
  }
}

#[derive(Debug, Clone)]
pub enum Event {
  Device(DeviceEvent),
  Gesture(GestureEvent),
  Keyboard(KeyboardEvent),
  Key(KeyKind),
  AddDocument(Box<Info>),
  Open(Box<Info>),
  OpenToc(Vec<TocEntry>, usize),
  LoadPixmap(usize),
  Update(UpdateMode),
  Invalid(Box<Info>),
  Notify(String),
  Page(CycleDir),
  ResultsPage(CycleDir),
  GoTo(usize),
  GoToLocation(Location),
  ResultsGoTo(usize),
  CropMargins(Box<Margin>),
  Chapter(CycleDir),
  Sort(SortMethod),
  SelectDirectory(PathBuf),
  ToggleSelectDirectory(PathBuf),
  NavigationBarResized(i32),
  Focus(Option<ViewId>),
  Select(EntryId),
  PropagateSelect(EntryId),
  EditLanguages,
  Define(String),
  Submit(ViewId, String),
  Slider(SliderId, f32, FingerStatus),
  ToggleNear(ViewId, Rectangle),
  ToggleInputHistoryMenu(ViewId, Rectangle),
  ToggleBookMenu(Rectangle, usize),
  TogglePresetMenu(Rectangle, usize),
  SubMenu(Rectangle, Vec<EntryKind>),
  ProcessLine(LineOrigin, String),
  History(CycleDir, bool),
  Toggle(ViewId),
  Show(ViewId),
  Close(ViewId),
  CloseSub(ViewId),
  Search(String),
  SearchResult(usize, Vec<Boundary>),
  EndOfSearch,
  Finished,
  ClockTick,
  BatteryTick,
  ToggleFrontlight,
  Load(PathBuf),
  LoadPreset(usize),
  Scroll(i32),
  Save,
  Guess,
  CheckBattery,
  SetWifi(bool),
  MightSuspend,
  PrepareSuspend,
  Suspend,
  Share,
  PrepareShare,
  Validate,
  Cancel,
  Reseed,
  Back,
  Quit,
  WakeUp,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum AppCmd {
  Sketch,
  Calculator,
  Dictionary { query: String, language: String },
}

#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq)]
pub enum ViewId {
  Home,
  Reader,
  SortMenu,
  MainMenu,
  TitleMenu,
  SelectionMenu,
  AnnotationMenu,
  BatteryMenu,
  ClockMenu,
  SearchTargetMenu,
  InputHistoryMenu,
  KeyboardLayoutMenu,
  Frontlight,
  Dictionary,
  FontSizeMenu,
  TextAlignMenu,
  FontFamilyMenu,
  MarginWidthMenu,
  ContrastExponentMenu,
  ContrastGrayMenu,
  LineHeightMenu,
  DirectoryMenu,
  BookMenu,
  LibraryMenu,
  PageMenu,
  PresetMenu,
  MarginCropperMenu,
  SearchMenu,
  SketchMenu,
  GoToPage,
  GoToPageInput,
  GoToResultsPage,
  GoToResultsPageInput,
  NamePage,
  NamePageInput,
  EditNote,
  EditNoteInput,
  EditLanguages,
  EditLanguagesInput,
  HomeSearchInput,
  ReaderSearchInput,
  DictionarySearchInput,
  CalculatorInput,
  SearchBar,
  AddressBar,
  AddressBarInput,
  Keyboard,
  AboutDialog,
  ShareDialog,
  MarginCropper,
  TopBottomBars,
  TableOfContents,
  MessageNotif,
  BoundaryNotif,
  TakeScreenshotNotif,
  SaveDocumentNotif,
  SaveSketchNotif,
  LoadSketchNotif,
  NoSearchResultsNotif,
  InvalidSearchQueryNotif,
  LowBatteryNotif,
  NetUpNotif,
  SubMenu(u8),
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum SliderId {
  FontSize,
  LightIntensity,
  LightWarmth,
  ContrastExponent,
  ContrastGray,
}

impl SliderId {
  pub fn label(self) -> String {
    match self {
      SliderId::LightIntensity => "Intensity".to_string(),
      SliderId::LightWarmth => "Warmth".to_string(),
      SliderId::FontSize => "Font Size".to_string(),
      SliderId::ContrastExponent => "Contrast Exponent".to_string(),
      SliderId::ContrastGray => "Contrast Gray".to_string(),
    }
  }
}

#[derive(Debug, Clone)]
pub enum Align {
  Left(i32),
  Right(i32),
  Center,
}

impl Align {
  #[inline]
  pub fn offset(&self, width: i32, container_width: i32) -> i32 {
    match *self {
      Align::Left(dx) => dx,
      Align::Right(dx) => container_width - width - dx,
      Align::Center => (container_width - width) / 2,
    }
  }
}

#[derive(Debug, Copy, Clone)]
pub enum KeyboardEvent {
  Append(char),
  Partial(char),
  Move { target: TextKind, dir: LinearDir },
  Delete { target: TextKind, dir: LinearDir },
  Submit,
}

#[derive(Debug, Copy, Clone)]
pub enum TextKind {
  Char,
  Word,
  Extremum,
}

#[derive(Debug, Clone)]
pub enum EntryKind {
  Message(String),
  Command(String, EntryId),
  CheckBox(String, EntryId, bool),
  RadioButton(String, EntryId, bool),
  SubMenu(String, Vec<EntryKind>),
  Separator,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum EntryId {
  About,
  SystemInfo,
  LoadLibrary(usize),
  Load(PathBuf),
  Flush,
  Save,
  Import,
  CleanUp,
  Sort(SortMethod),
  ReverseOrder,
  Remove(PathBuf),
  MoveTo(PathBuf, usize),
  AddDirectory(PathBuf),
  SelectDirectory(PathBuf),
  ToggleSelectDirectory(PathBuf),
  SetStatus(PathBuf, SimpleStatus),
  ToggleIntermissionImage(IntermKind, PathBuf),
  RemovePreset(usize),
  FirstColumn(FirstColumn),
  SecondColumn(SecondColumn),
  ApplyCroppings(usize, PageScheme),
  RemoveCroppings,
  SetZoomMode(ZoomMode),
  SetPageName,
  RemovePageName,
  HighlightSelection,
  AnnotateSelection,
  DefineSelection,
  SearchForSelection,
  AdjustSelection,
  RemoveAnnotation([TextLocation; 2]),
  EditAnnotationNote([TextLocation; 2]),
  RemoveAnnotationNote([TextLocation; 2]),
  GoTo(usize),
  GoToSelectedPageName,
  SearchDirection(LinearDir),
  SetButtonScheme(ButtonScheme),
  SetFontFamily(String),
  SetFontSize(i32),
  SetTextAlign(TextAlign),
  SetMarginWidth(i32),
  SetLineHeight(i32),
  SetContrastExponent(i32),
  SetContrastGray(i32),
  SetRotationLock(Option<RotationLock>),
  SetSearchTarget(Option<String>),
  SetInputText(ViewId, String),
  SetKeyboardLayout(String),
  ToggleShowHidden,
  ToggleFuzzy,
  ToggleInverted,
  ToggleMonochrome,
  ToggleWifi,
  Rotate(i8),
  Launch(AppCmd),
  SetPenSize(i32),
  SetPenColor(u8),
  TogglePenDynamism,
  ReloadDictionaries,
  New,
  Refresh,
  TakeScreenshot,
  Reboot,
  RebootInNickel,
  Quit,
}

impl EntryKind {
  pub fn is_separator(&self) -> bool {
    match *self {
      EntryKind::Separator => true,
      _ => false,
    }
  }

  pub fn text(&self) -> &str {
    match *self {
      EntryKind::Message(ref s)
      | EntryKind::Command(ref s, ..)
      | EntryKind::CheckBox(ref s, ..)
      | EntryKind::RadioButton(ref s, ..)
      | EntryKind::SubMenu(ref s, ..) => s,
      _ => "",
    }
  }

  pub fn get(&self) -> Option<bool> {
    match *self {
      EntryKind::CheckBox(_, _, v) | EntryKind::RadioButton(_, _, v) => Some(v),
      _ => None,
    }
  }

  pub fn set(&mut self, value: bool) {
    match *self {
      EntryKind::CheckBox(_, _, ref mut v) | EntryKind::RadioButton(_, _, ref mut v) => *v = value,
      _ => (),
    }
  }
}

pub struct RenderData {
  pub id: Option<Id>,
  pub rect: Rectangle,
  pub mode: UpdateMode,
  pub wait: bool,
}

impl RenderData {
  pub fn new(id: Id, rect: Rectangle, mode: UpdateMode) -> RenderData {
    RenderData {
      id: Some(id),
      rect,
      mode,
      wait: true,
    }
  }

  pub fn no_wait(id: Id, rect: Rectangle, mode: UpdateMode) -> RenderData {
    RenderData {
      id: Some(id),
      rect,
      mode,
      wait: false,
    }
  }

  pub fn expose(rect: Rectangle, mode: UpdateMode) -> RenderData {
    RenderData {
      id: None,
      rect,
      mode,
      wait: true,
    }
  }
}

type RQ = FxHashMap<(UpdateMode, bool), Vec<(Option<Id>, Rectangle)>>;
pub struct RenderQueue(RQ);

impl RenderQueue {
  pub fn new() -> RenderQueue {
    RenderQueue(FxHashMap::default())
  }

  pub fn add(&mut self, data: RenderData) {
    self
      .entry((data.mode, data.wait))
      .or_insert_with(|| Vec::new())
      .push((data.id, data.rect));
  }
}

impl Deref for RenderQueue {
  type Target = RQ;

  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

impl DerefMut for RenderQueue {
  fn deref_mut(&mut self) -> &mut Self::Target {
    &mut self.0
  }
}

pub static ID_FEEDER: IdFeeder = IdFeeder::new(1);
pub struct IdFeeder(AtomicU64);
pub type Id = u64;

impl IdFeeder {
  pub const fn new(id: Id) -> Self {
    IdFeeder(AtomicU64::new(id))
  }

  pub fn next(&self) -> Id {
    self.0.fetch_add(1, Ordering::Relaxed)
  }
}
