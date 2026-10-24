use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use gtk::cairo::{Context, FontSlant, FontWeight};
use gtk::gdk;
use gtk::prelude::*;
use gtk4 as gtk;
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};

use crate::core::{
    Color, KeyboardGeometry, KeyboardLayout, Modifiers, Palette, Rect, actions_for, chord_label,
    is_mouse_key,
};
use crate::icons::{ActionIcon, IconAtlas, icons_for_actions, with_icon_atlas};
use crate::peripherals::{PeripheralRow, touchpad_rows};
use crate::search_style::{ControlStyle, SearchStyle};
use crate::system::{RuntimeSnapshot, load_runtime};

#[derive(Clone, Debug)]
struct HitRegion {
    rect: Rect,
    key: String,
    legend: String,
}

#[derive(Clone, Debug, Default)]
struct RenderMetrics {
    panel: Rect,
    panels: Vec<Rect>,
    mouse: Rect,
    search: ScrollMetrics,
    touchpad: ScrollMetrics,
    pointer: Option<(f64, f64)>,
    rows: Vec<(Rect, String)>,
    hits: Vec<HitRegion>,
}

#[derive(Clone, Debug, Default)]
struct ScrollMetrics {
    rect: Rect,
    offset: f64,
    maximum: f64,
}

impl RenderMetrics {
    fn contains_panel(&self, x: f64, y: f64) -> bool {
        self.panels.iter().any(|panel| panel.contains(x, y))
    }

    fn scroll(&mut self, delta: f64) -> bool {
        let Some((x, y)) = self.pointer else {
            return false;
        };
        for area in [&mut self.search, &mut self.touchpad] {
            if area.rect.width > 0.0 && area.rect.contains(x, y) {
                area.offset = (area.offset + delta * 40.0).clamp(0.0, area.maximum);
                return true;
            }
        }
        false
    }
}

struct OverlayState {
    search_style: SearchStyle,
    native_entry: bool,
    snapshot: RuntimeSnapshot,
    geometry: KeyboardGeometry,
    latched_modifiers: Modifiers,
    held_modifiers: Modifiers,
    modifiers: Modifiers,
    query: String,
    hovered: Option<String>,
    metrics: RenderMetrics,
}

#[derive(Clone, Copy)]
struct ObservedModifierMasks {
    event: gdk::ModifierType,
    device: Option<gdk::ModifierType>,
}

impl ObservedModifierMasks {
    fn capture(controller: &gtk::EventControllerKey, event: gdk::ModifierType) -> Self {
        Self {
            event,
            device: controller
                .current_event_device()
                .map(|device| device.modifier_state()),
        }
    }
}

pub fn run(layout_override: Option<KeyboardLayout>) -> u8 {
    let application = gtk::Application::builder()
        .application_id("dev.keywrld.Keywrld")
        .build();
    let current_window = Rc::new(RefCell::new(None::<gtk::ApplicationWindow>));

    application.connect_activate({
        let current_window = current_window.clone();
        move |application| {
            let existing = current_window.borrow_mut().take();
            if let Some(window) = existing {
                window.close();
                return;
            }

            let snapshot = load_runtime(layout_override);
            let window = build_window(application, snapshot);
            window.connect_close_request({
                let current_window = current_window.clone();
                move |_| {
                    current_window.borrow_mut().take();
                    gtk::glib::Propagation::Proceed
                }
            });
            current_window.borrow_mut().replace(window.clone());
            window.present();
        }
    });

    application.run_with_args(&["keywrld"]).get()
}

fn build_window(
    application: &gtk::Application,
    snapshot: RuntimeSnapshot,
) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title("Keywrld")
        .decorated(false)
        .build();
    window.add_css_class("keywrld-overlay");
    window.init_layer_shell();
    window.set_namespace(Some("keywrld"));
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.set_exclusive_zone(0);
    for edge in [
        gtk4_layer_shell::Edge::Top,
        gtk4_layer_shell::Edge::Right,
        gtk4_layer_shell::Edge::Bottom,
        gtk4_layer_shell::Edge::Left,
    ] {
        window.set_anchor(edge, true);
    }
    select_monitor(&window, snapshot.monitor_connector.as_deref());

    let search_style = SearchStyle::load(&window);
    let provider = gtk::CssProvider::new();
    provider.load_from_data(&search_style.css());
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        window.connect_close_request({
            let display = display.clone();
            let provider = provider.clone();
            move |_| {
                gtk::style_context_remove_provider_for_display(&display, &provider);
                gtk::glib::Propagation::Proceed
            }
        });
        window.connect_destroy(move |_| {
            gtk::style_context_remove_provider_for_display(&display, &provider);
        });
    }

    let state = Rc::new(RefCell::new(OverlayState {
        search_style,
        native_entry: true,
        geometry: KeyboardGeometry::new(snapshot.layout),
        snapshot,
        latched_modifiers: Modifiers::default(),
        held_modifiers: Modifiers::default(),
        modifiers: Modifiers::default(),
        query: String::new(),
        hovered: None,
        metrics: RenderMetrics::default(),
    }));

    let drawing_area = gtk::DrawingArea::builder()
        .hexpand(true)
        .vexpand(true)
        .focusable(false)
        .build();
    drawing_area.set_draw_func({
        let state = state.clone();
        move |area, context, width, height| {
            let mut state = state.borrow_mut();
            draw_overlay(context, width as f64, height as f64, &mut state);
            area.set_tooltip_text(tooltip_at_pointer(&state).as_deref());
        }
    });

    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion({
        let state = state.clone();
        let drawing_area = drawing_area.clone();
        move |_, x, y| {
            let hovered = state
                .borrow()
                .metrics
                .hits
                .iter()
                .find(|hit| hit.rect.contains(x, y))
                .map(|hit| hit.key.clone());
            let mut state = state.borrow_mut();
            let search_hover_changed = state.metrics.search.rect.contains(x, y)
                || state
                    .metrics
                    .pointer
                    .is_some_and(|(x, y)| state.metrics.search.rect.contains(x, y));
            state.metrics.pointer = Some((x, y));
            drawing_area.set_tooltip_text(tooltip_at_pointer(&state).as_deref());
            if state.hovered != hovered || search_hover_changed {
                state.hovered = hovered;
                drawing_area.queue_draw();
            }
        }
    });
    motion.connect_leave({
        let state = state.clone();
        let drawing_area = drawing_area.clone();
        move |_| {
            state.borrow_mut().metrics.pointer = None;
            drawing_area.set_tooltip_text(None);
            state.borrow_mut().hovered = None;
            drawing_area.queue_draw();
        }
    });
    drawing_area.add_controller(motion);

    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    scroll.connect_scroll({
        let state = state.clone();
        let drawing_area = drawing_area.clone();
        move |_, _, dy| {
            if state.borrow_mut().metrics.scroll(dy) {
                drawing_area.set_tooltip_text(None);
                drawing_area.queue_draw();
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        }
    });
    drawing_area.add_controller(scroll);

    let click = gtk::GestureClick::new();
    click.connect_released({
        let state = state.clone();
        let drawing_area = drawing_area.clone();
        let window = window.downgrade();
        move |_, _, x, y| {
            let clicked_modifier = state
                .borrow()
                .metrics
                .hits
                .iter()
                .find(|hit| hit.rect.contains(x, y))
                .and_then(|hit| modifier_for_key_id(&hit.key));
            if let Some(modifier) = clicked_modifier {
                toggle_latched_modifier(&state, &drawing_area, modifier);
            } else if !state.borrow().metrics.contains_panel(x, y)
                && let Some(window) = window.upgrade()
            {
                window.close();
            }
        }
    });
    drawing_area.add_controller(click);

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed({
        let state = state.clone();
        let drawing_area = drawing_area.clone();
        let window = window.downgrade();
        move |_, key, _, _| {
            let before = state.borrow().held_modifiers;
            let candidate = modifiers_for_key_event(before, key, true);
            if key == gdk::Key::Escape {
                if let Some(window) = window.upgrade() {
                    window.close();
                }
                return gtk::glib::Propagation::Stop;
            }
            if let Some(modifiers) = candidate {
                update_held_modifiers(&state, &drawing_area, modifiers);
            }
            gtk::glib::Propagation::Proceed
        }
    });
    keys.connect_key_released({
        let state = state.clone();
        let drawing_area = drawing_area.clone();
        move |_, key, _, _| {
            let before = state.borrow().held_modifiers;
            let candidate = modifiers_for_key_event(before, key, false);
            if let Some(modifiers) = candidate {
                update_held_modifiers(&state, &drawing_area, modifiers);
            }
        }
    });
    keys.connect_modifiers({
        let state = state.clone();
        let drawing_area = drawing_area.clone();
        move |controller, modifier_state| {
            let masks = ObservedModifierMasks::capture(controller, modifier_state);
            let current = state.borrow().held_modifiers;
            let modifiers = modifiers_from_state_change(masks.event, masks.device, current);
            update_held_modifiers(&state, &drawing_area, modifiers);
            gtk::glib::Propagation::Proceed
        }
    });
    window.add_controller(keys);

    let search = gtk::Entry::builder()
        .placeholder_text("Search shortcuts…")
        .width_chars(1)
        .secondary_icon_name("edit-clear-symbolic")
        .build();
    search.add_css_class("shortcut-search");
    search.set_icon_tooltip_text(gtk::EntryIconPosition::Secondary, Some("Clear search"));
    search.connect_icon_release(|entry, position| {
        if position == gtk::EntryIconPosition::Secondary {
            entry.set_text("");
            entry.grab_focus();
        }
    });
    search.connect_changed({
        let state = state.clone();
        let drawing_area = drawing_area.clone();
        move |entry| {
            let mut state = state.borrow_mut();
            state.query = entry.text().to_string();
            state.metrics.search.offset = 0.0;
            drawing_area.set_tooltip_text(None);
            drawing_area.queue_draw();
        }
    });
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&drawing_area));
    overlay.add_overlay(&search);
    overlay.connect_get_child_position({
        let state = state.clone();
        move |overlay, _| {
            let state = state.borrow();
            let layout = overlay_layout(overlay.width() as f64, overlay.height() as f64, &state);
            let rect = search_entry_rect(layout.search, &state.search_style);
            Some(gdk::Rectangle::new(
                rect.x.round() as i32,
                rect.y.round() as i32,
                rect.width.round() as i32,
                rect.height.round() as i32,
            ))
        }
    });
    window.set_child(Some(&overlay));
    gtk::prelude::GtkWindowExt::set_focus(&window, Some(&search));
    window
}

fn tooltip_at_pointer(state: &OverlayState) -> Option<String> {
    let (x, y) = state.metrics.pointer?;
    state
        .metrics
        .hits
        .iter()
        .find(|hit| hit.rect.contains(x, y) && is_mouse_key(&hit.key))
        .map(|hit| {
            mouse_tooltip(
                hit,
                &actions_for(&state.snapshot.bindings, state.modifiers),
                state.modifiers,
            )
        })
        .or_else(|| {
            state
                .metrics
                .rows
                .iter()
                .find(|(rect, _)| rect.contains(x, y))
                .map(|(_, text)| text.clone())
        })
}

fn select_monitor(window: &gtk::ApplicationWindow, connector: Option<&str>) {
    let (Some(connector), Some(display)) = (connector, gdk::Display::default()) else {
        return;
    };
    let monitors = display.monitors();
    for index in 0..monitors.n_items() {
        let Some(object) = monitors.item(index) else {
            continue;
        };
        let Ok(monitor) = object.downcast::<gdk::Monitor>() else {
            continue;
        };
        if monitor.connector().as_deref() == Some(connector) {
            window.set_monitor(Some(&monitor));
            return;
        }
    }
}

fn modifier_for_key(key: gdk::Key) -> Option<Modifiers> {
    match key {
        gdk::Key::Shift_L | gdk::Key::Shift_R => Some(Modifiers::SHIFT),
        gdk::Key::Control_L | gdk::Key::Control_R => Some(Modifiers::CTRL),
        gdk::Key::Alt_L | gdk::Key::Alt_R => Some(Modifiers::ALT),
        gdk::Key::Super_L | gdk::Key::Super_R => Some(Modifiers::SUPER),
        _ => None,
    }
}

fn modifiers_from_gdk(state: gdk::ModifierType) -> Modifiers {
    let mut modifiers = Modifiers::default();
    if state.contains(gdk::ModifierType::SHIFT_MASK) {
        modifiers = modifiers.with(Modifiers::SHIFT);
    }
    if state.contains(gdk::ModifierType::CONTROL_MASK) {
        modifiers = modifiers.with(Modifiers::CTRL);
    }
    if state.contains(gdk::ModifierType::ALT_MASK) {
        modifiers = modifiers.with(Modifiers::ALT);
    }
    modifiers
}

fn modifiers_from_state_change(
    signal_state: gdk::ModifierType,
    device_state: Option<gdk::ModifierType>,
    current: Modifiers,
) -> Modifiers {
    let observed = device_state.unwrap_or(signal_state);
    let modifiers = modifiers_from_gdk(observed);
    if current.contains(Modifiers::SUPER) && observed.contains(gdk::ModifierType::SUPER_MASK) {
        modifiers.with(Modifiers::SUPER)
    } else {
        modifiers
    }
}

fn modifiers_for_key_event(current: Modifiers, key: gdk::Key, pressed: bool) -> Option<Modifiers> {
    match (modifier_for_key(key), pressed) {
        (Some(modifier), true) => Some(current.with(modifier)),
        (Some(modifier), false) => Some(current.without(modifier)),
        (None, _) => None,
    }
}

fn update_held_modifiers(
    state: &Rc<RefCell<OverlayState>>,
    drawing_area: &gtk::DrawingArea,
    held_modifiers: Modifiers,
) {
    let mut state = state.borrow_mut();
    let modifiers = state.latched_modifiers.with(held_modifiers);
    if state.modifiers != modifiers || state.held_modifiers != held_modifiers {
        state.held_modifiers = held_modifiers;
        state.modifiers = modifiers;
        state.metrics.search.offset = 0.0;
        state.hovered = None;
        drawing_area.set_tooltip_text(None);
        drawing_area.queue_draw();
    }
}

fn toggle_latched_modifier(
    state: &Rc<RefCell<OverlayState>>,
    drawing_area: &gtk::DrawingArea,
    modifier: Modifiers,
) {
    let mut state = state.borrow_mut();
    state.latched_modifiers = state.latched_modifiers.toggled(modifier);
    state.modifiers = state.latched_modifiers.with(state.held_modifiers);
    state.metrics.search.offset = 0.0;
    state.hovered = None;
    drawing_area.set_tooltip_text(None);
    drawing_area.queue_draw();
}

fn draw_overlay(context: &Context, width: f64, height: f64, state: &mut OverlayState) {
    let palette = state.snapshot.palette;
    set_source(context, palette.darker_background.with_alpha(0.82));
    let _ = context.paint();

    let actions = actions_for(&state.snapshot.bindings, state.modifiers);
    // Keep the diagram geometry stable while only its actions follow modifiers.
    let mouse_keys: BTreeSet<_> = state
        .snapshot
        .bindings
        .iter()
        .filter(|binding| is_mouse_key(&binding.key))
        .map(|binding| binding.key.clone())
        .collect();

    let touch_rows = if state.snapshot.pointing.touchpads.is_empty() {
        Vec::new()
    } else {
        touchpad_rows(&state.snapshot.pointing, &state.snapshot.bindings)
    };
    let layout = overlay_layout(width, height, state);
    let panel = layout.keyboard;
    let (outer_padding, header_height, detail_height) = layout.chrome();
    let unit = layout.unit;
    let board_width = state.geometry.width * unit;
    let board_height = state.geometry.height * unit;
    let old_panels = state.metrics.panels.clone();
    state.metrics.panel = panel;
    state.metrics.panels = vec![panel, layout.search];
    state.metrics.panels.extend(layout.mouse);
    state.metrics.panels.extend(layout.touchpad);
    state.metrics.hits.clear();
    state.metrics.rows.clear();
    if old_panels != state.metrics.panels {
        state.hovered = state.metrics.pointer.and_then(|(x, y)| {
            layout
                .keyboard_hits(&state.geometry)
                .into_iter()
                .chain(
                    layout
                        .mouse
                        .into_iter()
                        .flat_map(|panel| mouse_geometry(panel, &mouse_keys).1),
                )
                .find(|hit| hit.rect.contains(x, y))
                .map(|hit| hit.key)
        });
    }

    rounded_rectangle(context, panel, 18.0);
    set_source(context, palette.background.with_alpha(0.985));
    let _ = context.fill_preserve();
    context.set_line_width(1.0);
    set_source(context, palette.muted.with_alpha(0.9));
    let _ = context.stroke();

    let content_x = panel.x + outer_padding;
    let board_y = panel.y + outer_padding + header_height;
    draw_header(
        context,
        state,
        Rect {
            x: content_x,
            y: panel.y + outer_padding,
            width: board_width,
            height: header_height,
        },
        palette,
    );

    for hit in layout.keyboard_hits(&state.geometry) {
        let rect = hit.rect;
        let key = &hit.key;
        let key_actions = actions.get(key);
        let hovered = state.hovered.as_deref() == Some(key.as_str());
        let modifier_active = modifier_key_active(key, state.modifiers);
        with_icon_atlas(|icon_atlas| {
            draw_key(
                context,
                rect,
                &hit.legend,
                key_actions,
                hovered,
                modifier_active,
                icon_atlas,
                palette,
            );
        });
        state.metrics.hits.push(hit);
    }

    draw_search_panel(context, layout.search, state);

    if let Some(panel) = layout.mouse {
        state.metrics.mouse = panel;
        draw_mouse_panel(context, panel, &mouse_keys, &actions, state);
    } else {
        state.metrics.mouse = Rect::default();
    }

    let detail_y = board_y + board_height + 10.0;
    draw_details(
        context,
        state,
        &actions,
        Rect {
            x: content_x,
            y: detail_y,
            width: board_width,
            height: detail_height - 10.0,
        },
        palette,
    );

    if let Some(panel) = layout.touchpad {
        draw_peripheral_panel(
            context,
            panel,
            "TOUCHPAD",
            "Clicks, scrolling and configured gestures",
            &touch_rows,
            &mut state.metrics.touchpad,
            &mut state.metrics.rows,
            palette,
        );
    } else {
        state.metrics.touchpad = ScrollMetrics::default();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayoutMode {
    Wide,
    Portrait,
    Compact,
}

#[derive(Debug)]
struct PanelLayout {
    mode: LayoutMode,
    keyboard: Rect,
    search: Rect,
    mouse: Option<Rect>,
    touchpad: Option<Rect>,
    unit: f64,
}

impl PanelLayout {
    fn chrome(&self) -> (f64, f64, f64) {
        if self.mode == LayoutMode::Compact {
            (16.0, 60.0, 48.0)
        } else {
            (24.0, 82.0, 66.0)
        }
    }

    fn keyboard_hits(&self, geometry: &KeyboardGeometry) -> Vec<HitRegion> {
        let (padding, header, _) = self.chrome();
        let gap = (self.unit * 0.075).clamp(3.0, 7.0).min(self.unit * 0.3);
        geometry
            .keys
            .iter()
            .map(|key| HitRegion {
                rect: Rect {
                    x: self.keyboard.x + padding + key.rect.x * self.unit + gap / 2.0,
                    y: self.keyboard.y + padding + header + key.rect.y * self.unit + gap / 2.0,
                    width: key.rect.width * self.unit - gap,
                    height: key.rect.height * self.unit - gap,
                },
                key: key.id.clone(),
                legend: key.legend.clone(),
            })
            .collect()
    }
}

fn overlay_layout(width: f64, height: f64, state: &OverlayState) -> PanelLayout {
    let show_mouse = !state.snapshot.pointing.mice.is_empty()
        || state
            .snapshot
            .bindings
            .iter()
            .any(|binding| is_mouse_key(&binding.key));
    let touch_count = if state.snapshot.pointing.touchpads.is_empty() {
        0
    } else {
        touchpad_rows(&state.snapshot.pointing, &state.snapshot.bindings).len()
    };
    panel_layout(
        width,
        height,
        &state.geometry,
        show_mouse,
        touch_count,
        state.snapshot.pointing.mouse_left,
    )
}

fn search_entry_rect(panel: Rect, style: &SearchStyle) -> Rect {
    Rect {
        x: panel.x + 12.0,
        y: panel.y + 18.0 + style.heading * 1.5,
        width: (panel.width - 24.0).max(1.0),
        height: style.entry_height(),
    }
}

// Compare exact, contiguous, then scattered matches. Character offsets keep
// ranking consistent for Unicode descriptions as well as ASCII key names.
fn fuzzy_token_score(field: &str, token: &str) -> Option<(usize, usize, usize)> {
    if field == token {
        return Some((0, 0, 0));
    }
    if let Some(start) = field.find(token) {
        return Some((0, 1, field[..start].chars().count()));
    }
    let mut wanted = token.chars();
    let mut next = wanted.next()?;
    for (index, character) in field.chars().enumerate() {
        if character == next {
            match wanted.next() {
                Some(character) => next = character,
                None => return Some((1, 1, index + 1 - token.chars().count())),
            }
        }
    }
    None
}

fn shortcut_rows(
    bindings: &[crate::core::Binding],
    modifiers: Modifiers,
    query: &str,
) -> Vec<PeripheralRow> {
    let tokens: Vec<_> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut seen = BTreeSet::new();
    let mut matches = Vec::new();
    for binding in bindings {
        if (!modifiers.is_empty() && binding.modifiers != modifiers)
            || !seen.insert((binding.modifiers, &binding.key, &binding.action))
        {
            continue;
        }
        let label = chord_label(binding.modifiers, &friendly_key_name(&binding.key));
        let fields = [
            binding.action.to_lowercase(),
            binding.key.to_lowercase(),
            label.to_lowercase(),
        ];
        let score = tokens.iter().try_fold((0, 0, 0), |total, token| {
            fields
                .iter()
                .filter_map(|field| fuzzy_token_score(field, token))
                .min()
                .map(|score| (total.0 + score.0, total.1 + score.1, total.2 + score.2))
        });
        if let Some(score) = score {
            matches.push((score, label, binding.action.clone()));
        }
    }
    matches.sort();
    matches
        .into_iter()
        .map(|(_, label, action)| PeripheralRow { label, action })
        .collect()
}

// Search uses its own font and measured widths; other panels retain their typography.
fn search_text(
    context: &Context,
    style: &SearchStyle,
    rect: Rect,
    text: &str,
    size: f64,
    weight: FontWeight,
    color: Color,
) {
    context.select_font_face(&style.font, FontSlant::Normal, weight);
    context.set_font_size(size);
    let fits = |text: &str| {
        context
            .text_extents(text)
            .is_ok_and(|e| e.x_advance() <= rect.width)
    };
    let mut text = text.to_string();
    if !fits(&text) {
        while text.pop().is_some() {
            if fits(&format!("{text}…")) {
                text.push('…');
                break;
            }
        }
    }
    let _ = context.save();
    context.rectangle(rect.x, rect.y, rect.width.max(0.0), rect.height.max(0.0));
    context.clip();
    let baseline = context
        .font_extents()
        .map(|e| (rect.height - e.height()) / 2.0 + e.ascent())
        .unwrap_or(size);
    set_source(context, color);
    context.move_to(rect.x, rect.y + baseline);
    let _ = context.show_text(&text);
    let _ = context.restore();
}

fn search_surface(context: &Context, rect: Rect, radius: f64, control: ControlStyle) {
    rounded_rectangle(
        context,
        rect,
        radius.min(rect.height / 2.0).min(rect.width / 2.0).max(0.0),
    );
    set_source(context, control.fill);
    let _ = context.fill();
    if control.width > 0.0 {
        let inset = control.width / 2.0;
        rounded_rectangle(
            context,
            Rect {
                x: rect.x + inset,
                y: rect.y + inset,
                width: (rect.width - control.width).max(0.0),
                height: (rect.height - control.width).max(0.0),
            },
            (radius - inset)
                .max(0.0)
                .min((rect.height - control.width).max(0.0) / 2.0),
        );
        context.set_line_width(control.width);
        set_source(context, control.border);
        let _ = context.stroke();
    }
}

fn draw_search_panel(context: &Context, panel: Rect, state: &mut OverlayState) {
    let style = &state.search_style;
    search_surface(
        context,
        panel,
        style.radius,
        ControlStyle {
            fill: style.background,
            border: style.border,
            width: style.border_width,
        },
    );
    search_text(
        context,
        style,
        Rect {
            x: panel.x + 18.0,
            y: panel.y + 12.0,
            width: (panel.width - 36.0).max(0.0),
            height: style.heading * 1.5,
        },
        "SHORTCUTS",
        style.heading,
        FontWeight::Bold,
        style.foreground,
    );
    let entry = search_entry_rect(panel, style);
    // Live GTK draws the field. Previews draw the focused field from the same tokens.
    if !state.native_entry {
        search_surface(context, entry, style.radius, style.controls[2]);
        let inset = style.padding_x + style.controls[2].width;
        search_text(
            context,
            style,
            Rect {
                x: entry.x + inset,
                y: entry.y,
                width: (entry.width - inset * 2.0 - style.body * 2.0).max(0.0),
                height: entry.height,
            },
            if state.query.is_empty() {
                "Search shortcuts…"
            } else {
                &state.query
            },
            style.body,
            FontWeight::Normal,
            if state.query.is_empty() {
                style.secondary
            } else {
                style.foreground
            },
        );
        search_text(
            context,
            style,
            Rect {
                x: entry.x + entry.width - inset - style.body,
                y: entry.y,
                width: style.body,
                height: entry.height,
            },
            "×",
            style.body,
            FontWeight::Normal,
            style.secondary,
        );
    }
    let rows = shortcut_rows(&state.snapshot.bindings, state.modifiers, &state.query);
    let filter = if state.modifiers.is_empty() {
        "All modifiers".into()
    } else {
        state.modifiers.label()
    };
    let noun = if rows.len() == 1 { "result" } else { "results" };
    let subtitle_y = entry.y + entry.height + 8.0;
    search_text(
        context,
        style,
        Rect {
            x: panel.x + 18.0,
            y: subtitle_y,
            width: (panel.width - 36.0).max(0.0),
            height: style.caption * 1.5,
        },
        &format!("{} {noun} · {filter}", rows.len()),
        style.caption,
        FontWeight::Normal,
        style.secondary,
    );
    let viewport_y = subtitle_y + style.caption * 1.5 + 10.0;
    let viewport = Rect {
        x: panel.x + 12.0,
        y: viewport_y,
        width: (panel.width - 24.0).max(0.0),
        height: (panel.y + panel.height - 12.0 - viewport_y).max(0.0),
    };
    let scroll = &mut state.metrics.search;
    scroll.rect = viewport;
    let stride = style.row_height() + style.row_gap;
    let content_height = (rows.len() as f64 * stride - style.row_gap).max(0.0);
    scroll.maximum = (content_height - viewport.height).max(0.0);
    scroll.offset = scroll.offset.clamp(0.0, scroll.maximum);
    let _ = context.save();
    context.rectangle(viewport.x, viewport.y, viewport.width, viewport.height);
    context.clip();
    if rows.is_empty() {
        search_text(
            context,
            style,
            Rect {
                x: viewport.x + style.padding_x,
                y: viewport.y,
                width: (viewport.width - style.padding_x * 2.0).max(0.0),
                height: style.row_height(),
            },
            "No matching shortcuts",
            style.body,
            FontWeight::Normal,
            style.secondary,
        );
    }
    for (index, row) in rows.iter().enumerate() {
        let rect = Rect {
            x: viewport.x,
            y: viewport.y + index as f64 * stride - scroll.offset,
            width: (viewport.width - 8.0).max(0.0),
            height: style.row_height(),
        };
        if rect.y + rect.height <= viewport.y || rect.y >= viewport.y + viewport.height {
            continue;
        }
        let hovered = state
            .metrics
            .pointer
            .is_some_and(|(x, y)| viewport.contains(x, y) && rect.contains(x, y));
        if hovered {
            search_surface(context, rect, style.radius, style.row_hover);
        }
        let text_rect = Rect {
            x: rect.x + style.row_padding_x,
            y: rect.y + style.padding_y,
            width: (rect.width - style.row_padding_x * 2.0).max(0.0),
            height: style.caption * 1.5,
        };
        search_text(
            context,
            style,
            text_rect,
            &row.label,
            style.caption,
            FontWeight::Bold,
            style.accent,
        );
        search_text(
            context,
            style,
            Rect {
                y: text_rect.y + text_rect.height,
                height: style.body * 1.5,
                ..text_rect
            },
            &row.action,
            style.body,
            FontWeight::Normal,
            if hovered {
                style.row_hover_text
            } else {
                style.foreground
            },
        );
        let top = rect.y.max(viewport.y);
        let bottom = (rect.y + rect.height).min(viewport.y + viewport.height);
        state.metrics.rows.push((
            Rect {
                y: top,
                height: bottom - top,
                ..rect
            },
            format!("{}\n{}", row.label, row.action),
        ));
    }
    if scroll.maximum > 0.0 && viewport.height > 0.0 {
        let height = (viewport.height * viewport.height / content_height)
            .max(16.0)
            .min(viewport.height);
        let thumb = Rect {
            x: viewport.x + viewport.width - 4.0,
            y: viewport.y + (viewport.height - height) * scroll.offset / scroll.maximum,
            width: 3.0,
            height,
        };
        search_surface(
            context,
            thumb,
            style.radius.min(1.5),
            ControlStyle {
                fill: style.accent.with_alpha(0.65),
                border: style.accent,
                width: 0.0,
            },
        );
    }
    let _ = context.restore();
}

// GTK allocations are logical pixels; orientation follows the actual surface,
// including monitor rotation and scale, without consulting compositor settings.
fn panel_layout(
    width: f64,
    height: f64,
    geometry: &KeyboardGeometry,
    show_mouse: bool,
    touch_rows: usize,
    left: bool,
) -> PanelLayout {
    let wide = wide_panel_layout(width, height, geometry, show_mouse, touch_rows, left);
    if width >= height && wide.unit >= 32.0 {
        return wide;
    }
    let portrait = height > width;
    let gap = 24.0;
    let margin = (width.min(height) * 0.03).max(16.0);
    let available_width = (width - 2.0 * margin).max(1.0);
    let available_height = (height - 2.0 * margin).max(1.0);
    let has_touch = touch_rows > 0;
    let (padding, chrome) = if portrait {
        (24.0, 196.0)
    } else {
        (16.0, 140.0)
    };
    let lower_minimum = if portrait && show_mouse && has_touch {
        492.0
    } else {
        300.0
    };
    let unit = ((available_width - padding * 2.0) / geometry.width)
        .min((available_height - gap - lower_minimum - chrome) / geometry.height)
        .clamp(1.0, 78.0);
    let keyboard_width = geometry.width * unit + padding * 2.0;
    let keyboard_height = geometry.height * unit + chrome;
    let desired_lower = if portrait {
        let devices = if show_mouse { 450.0 } else { 0.0 }
            + if has_touch {
                (72.0 + touch_rows as f64 * 48.0).min(360.0)
            } else {
                0.0
            }
            + if show_mouse && has_touch { gap } else { 0.0 };
        devices.max(600.0)
    } else {
        300.0
    };
    let lower_height = desired_lower.min((available_height - keyboard_height - gap).max(1.0));
    // The support panels can be wider than a height-constrained keyboard.
    let group_width = available_width.min(geometry.width * 78.0 + padding * 2.0);
    let x = (width - group_width) / 2.0;
    let y = (height - keyboard_height - gap - lower_height) / 2.0;
    let keyboard = Rect {
        x: (width - keyboard_width) / 2.0,
        y,
        width: keyboard_width,
        height: keyboard_height,
    };
    let lower_y = y + keyboard_height + gap;
    let mut search = Rect {
        x,
        y: lower_y,
        width: group_width,
        height: lower_height,
    };
    let mut mouse = None;
    let mut touchpad = None;
    if portrait && (show_mouse || has_touch) {
        let device_width = (group_width * 0.44)
            .clamp(210.0, 360.0)
            .min((group_width - gap) / 2.0);
        search.width = group_width - gap - device_width;
        let device_x = if left && show_mouse {
            search.x = x + device_width + gap;
            x
        } else {
            x + search.width + gap
        };
        let mouse_height = if has_touch {
            (lower_height - gap - 168.0).min(450.0)
        } else {
            lower_height.min(450.0)
        };
        if show_mouse {
            mouse = Some(Rect {
                x: device_x,
                y: lower_y,
                width: device_width,
                height: mouse_height,
            });
        }
        if has_touch {
            let offset = if show_mouse { mouse_height + gap } else { 0.0 };
            touchpad = Some(Rect {
                x: device_x,
                y: lower_y + offset,
                width: device_width,
                height: lower_height - offset,
            });
        }
    } else if !portrait {
        let mouse_width = if show_mouse {
            (group_width * 0.26).clamp(210.0, 310.0)
        } else {
            0.0
        };
        let mouse_side = if show_mouse { mouse_width + gap } else { 0.0 };
        let list_width = group_width - mouse_side;
        search.width = if has_touch {
            (list_width - gap) / 2.0
        } else {
            list_width
        };
        let mut next_x = x + search.width + gap;
        if show_mouse && left {
            mouse = Some(Rect {
                x: next_x,
                y: lower_y,
                width: mouse_width,
                height: lower_height,
            });
            next_x += mouse_width + gap;
        }
        if has_touch {
            touchpad = Some(Rect {
                x: next_x,
                y: lower_y,
                width: search.width,
                height: lower_height,
            });
            next_x += search.width + gap;
        }
        if show_mouse && !left {
            mouse = Some(Rect {
                x: next_x,
                y: lower_y,
                width: mouse_width,
                height: lower_height,
            });
        }
    }
    PanelLayout {
        mode: if portrait {
            LayoutMode::Portrait
        } else {
            LayoutMode::Compact
        },
        keyboard,
        search,
        mouse,
        touchpad,
        unit,
    }
}

fn wide_panel_layout(
    width: f64,
    height: f64,
    geometry: &KeyboardGeometry,
    show_mouse: bool,
    touch_rows: usize,
    left: bool,
) -> PanelLayout {
    let gap = 24.0;
    let margin = (width.min(height) * 0.03).max(16.0);
    let available_height = (height - 2.0 * margin).max(1.0);
    let mouse_width = if !show_mouse {
        0.0
    } else {
        (width * 0.22).clamp(210.0, 310.0).min(width * 0.35)
    };
    let mouse_side = if !show_mouse { 0.0 } else { mouse_width + gap };
    let search_width = (width * 0.23).clamp(230.0, 340.0).min(width * 0.30);
    let search_side = search_width + gap;
    let side = search_side + mouse_side;
    let touch_height = if touch_rows == 0 {
        0.0
    } else {
        (72.0 + touch_rows as f64 * 48.0).min(available_height * 0.28)
    };
    let below = if touch_rows == 0 {
        0.0
    } else {
        touch_height + gap
    };
    let chrome = 82.0 + 66.0 + 48.0;
    let unit = ((width - 2.0 * margin - side - 48.0) / geometry.width)
        .min((available_height - below - chrome) / geometry.height)
        .clamp(1.0, 78.0);
    let keyboard_width = geometry.width * unit + 48.0;
    let keyboard_height = geometry.height * unit + chrome;
    let x = (width - keyboard_width - side) / 2.0;
    let keyboard = Rect {
        x: x + search_side + if left { mouse_side } else { 0.0 },
        y: (height - keyboard_height - below) / 2.0,
        width: keyboard_width,
        height: keyboard_height,
    };
    let mouse = show_mouse.then(|| {
        let panel_height = 450.0_f64.min(keyboard_height);
        Rect {
            x: if left {
                x + search_side
            } else {
                keyboard.x + keyboard.width + gap
            },
            y: keyboard.y + (keyboard.height - panel_height) / 2.0,
            width: mouse_width,
            height: panel_height,
        }
    });
    let touch_width = keyboard.width.min(720.0);
    let touchpad = (touch_rows > 0).then_some(Rect {
        x: keyboard.x + (keyboard.width - touch_width) / 2.0,
        y: keyboard.y + keyboard.height + gap,
        width: touch_width,
        height: touch_height,
    });
    PanelLayout {
        mode: LayoutMode::Wide,
        keyboard,
        search: Rect {
            x,
            y: keyboard.y,
            width: search_width,
            height: keyboard.height,
        },
        mouse,
        touchpad,
        unit,
    }
}

/// A normalized mouse silhouette; optional controls are based on all registered
/// bindings, never the currently selected chord.
fn mouse_geometry(panel: Rect, keys: &BTreeSet<String>) -> (Rect, Vec<HitRegion>) {
    let scale = ((panel.width - 32.0) / 220.0)
        .min((panel.height - 150.0) / 280.0)
        .max(0.1);
    let x = panel.x + (panel.width - 220.0 * scale) / 2.0;
    let y = panel.y + 66.0;
    let map = |x0, y0, width, height| Rect {
        x: x + x0 * scale,
        y: y + y0 * scale,
        width: width * scale,
        height: height * scale,
    };
    let mut controls = Vec::new();
    let mut add = |key: &str, legend: &str, x, y, width, height| {
        controls.push(HitRegion {
            key: key.into(),
            legend: legend.into(),
            rect: map(x, y, width, height),
        })
    };
    add("MOUSE_LEFT", "Left click", 34.0, 40.0, 56.0, 110.0);
    add("MOUSE_RIGHT", "Right click", 130.0, 40.0, 56.0, 110.0);
    add("MOUSE_UP", "Wheel up", 96.0, 47.0, 28.0, 28.0);
    add("MOUSE_MIDDLE", "Wheel click", 96.0, 79.0, 28.0, 32.0);
    add("MOUSE_DOWN", "Wheel down", 96.0, 115.0, 28.0, 28.0);
    for (key, legend, x) in [
        ("MOUSE_SCROLL_LEFT", "Wheel left", 62.0),
        ("MOUSE_SCROLL_RIGHT", "Wheel right", 124.0),
    ] {
        if keys.contains(key) {
            add(key, legend, x, 157.0, 34.0, 30.0);
        }
    }
    // Standard thumb buttons sit on the flank of the mouse.
    for (key, legend, y) in [
        ("MOUSE:275", "Button 4", 156.0),
        ("MOUSE:276", "Button 5", 202.0),
    ] {
        if keys.contains(key) {
            add(key, legend, 12.0, y, 18.0, 38.0);
        }
    }
    let represented: BTreeSet<_> = controls.iter().map(|hit| hit.key.as_str()).collect();
    let extra: Vec<_> = keys
        .iter()
        .filter(|key| !represented.contains(key.as_str()))
        .collect();
    let rows = extra.len().div_ceil(3).max(1);
    let cell_height = 60.0 / rows as f64;
    for (index, key) in extra.into_iter().enumerate() {
        controls.push(HitRegion {
            key: key.clone(),
            legend: friendly_key_name(key),
            rect: map(
                48.0 + (index % 3) as f64 * 43.0,
                202.0 + (index / 3) as f64 * cell_height,
                38.0,
                cell_height - 3.0,
            ),
        });
    }
    (map(26.0, 22.0, 168.0, 246.0), controls)
}

fn mouse_tooltip(
    hit: &HitRegion,
    actions: &BTreeMap<String, Vec<String>>,
    modifiers: Modifiers,
) -> String {
    format!(
        "{}\n{}",
        chord_label(modifiers, &hit.legend),
        actions
            .get(&hit.key)
            .map(|a| a.join(" · "))
            .unwrap_or_else(|| "No binding".into())
    )
}

fn draw_mouse_panel(
    context: &Context,
    panel: Rect,
    keys: &BTreeSet<String>,
    actions: &BTreeMap<String, Vec<String>>,
    state: &mut OverlayState,
) {
    let palette = state.snapshot.palette;
    rounded_rectangle(context, panel, 16.0);
    set_source(context, palette.background.with_alpha(0.985));
    let _ = context.fill_preserve();
    context.set_line_width(1.0);
    set_source(context, palette.muted.with_alpha(0.9));
    let _ = context.stroke();
    draw_text(
        context,
        panel.x + 18.0,
        panel.y + 29.0,
        "MOUSE",
        15.0,
        FontWeight::Bold,
        palette.foreground,
    );
    let subtitle = state
        .snapshot
        .pointing
        .mice
        .first()
        .map(String::as_str)
        .unwrap_or("Buttons and scroll wheel");
    draw_text(
        context,
        panel.x + 18.0,
        panel.y + 49.0,
        &elide(subtitle, ((panel.width - 36.0) / 6.0) as usize),
        11.0,
        FontWeight::Normal,
        palette.dark_foreground,
    );
    let (body, controls) = mouse_geometry(panel, keys);
    let cx = body.x + body.width / 2.0;
    // Cable and a rounded shell make the controls read as a mouse, not a grid.
    context.move_to(cx, body.y - 16.0);
    context.line_to(cx, body.y);
    context.set_line_width(2.0);
    set_source(context, palette.muted);
    let _ = context.stroke();
    let top_radius = body.width * 0.2;
    let bottom_radius = body.width * 0.5;
    let right = body.x + body.width;
    let bottom = body.y + body.height;
    context.new_sub_path();
    context.move_to(body.x + top_radius, body.y);
    context.line_to(right - top_radius, body.y);
    context.curve_to(right, body.y, right, body.y, right, body.y + top_radius);
    context.line_to(right, bottom - bottom_radius);
    context.curve_to(right, bottom, right, bottom, cx, bottom);
    context.curve_to(
        body.x,
        bottom,
        body.x,
        bottom,
        body.x,
        bottom - bottom_radius,
    );
    context.line_to(body.x, body.y + top_radius);
    context.curve_to(body.x, body.y, body.x, body.y, body.x + top_radius, body.y);
    context.close_path();
    set_source(context, palette.darker_background.with_alpha(0.65));
    let _ = context.fill_preserve();
    set_source(context, palette.muted);
    context.set_line_width(1.5);
    let _ = context.stroke();

    // Enclose the three wheel targets in one scroll-wheel housing.
    let up = controls
        .iter()
        .find(|hit| hit.key == "MOUSE_UP")
        .unwrap()
        .rect;
    let down = controls
        .iter()
        .find(|hit| hit.key == "MOUSE_DOWN")
        .unwrap()
        .rect;
    let housing = Rect {
        x: up.x - 4.0,
        y: up.y - 5.0,
        width: up.width + 8.0,
        height: down.y + down.height - up.y + 10.0,
    };
    rounded_rectangle(context, housing, housing.width / 2.0);
    set_source(context, palette.lighter_background);
    let _ = context.fill_preserve();
    set_source(context, palette.muted);
    context.set_line_width(1.0);
    let _ = context.stroke();

    let selected = if state.modifiers.is_empty() {
        "NO MODIFIERS".into()
    } else {
        state.modifiers.label()
    };
    draw_text_centered(
        context,
        cx,
        body.y + body.height * 0.71,
        &selected,
        (body.width / 18.0).clamp(7.0, 10.0),
        FontWeight::Bold,
        palette.dark_foreground,
    );
    for hit in &controls {
        let key_actions = actions.get(&hit.key);
        let hovered = state.hovered.as_deref() == Some(hit.key.as_str());
        let symbol = match hit.key.as_str() {
            "MOUSE_LEFT" => "L",
            "MOUSE_RIGHT" => "R",
            "MOUSE_UP" => "↑",
            "MOUSE_DOWN" => "↓",
            "MOUSE_MIDDLE" => "●",
            "MOUSE_SCROLL_LEFT" => "←",
            "MOUSE_SCROLL_RIGHT" => "→",
            "MOUSE:275" => "4",
            "MOUSE:276" => "5",
            _ => "•",
        };
        let _ = context.save();
        // Unknown/long action text must never spill into neighboring controls.
        context.rectangle(hit.rect.x, hit.rect.y, hit.rect.width, hit.rect.height);
        context.clip();
        with_icon_atlas(|atlas| {
            // A tiny wheel/side target cannot fit an unknown action's name;
            // its full text remains available in both details and the tooltip.
            let compact_summary = vec!["…".to_string()];
            let display_actions = if hit.rect.width < 40.0
                && key_actions.is_some_and(|a| icons_for_actions(a).is_none())
            {
                Some(&compact_summary)
            } else {
                key_actions
            };
            draw_key(
                context,
                hit.rect,
                symbol,
                display_actions,
                hovered,
                false,
                atlas,
                palette,
            )
        });
        let _ = context.restore();
    }
    let hovered = controls
        .iter()
        .find(|hit| state.hovered.as_deref() == Some(hit.key.as_str()));
    let footer = Rect {
        x: panel.x + 12.0,
        y: panel.y + panel.height - 78.0,
        width: panel.width - 24.0,
        height: 66.0,
    };
    rounded_rectangle(context, footer, 8.0);
    set_source(context, palette.darker_background.with_alpha(0.72));
    let _ = context.fill();
    if let Some(hit) = hovered {
        draw_text(
            context,
            footer.x + 10.0,
            footer.y + 18.0,
            &elide(
                &chord_label(state.modifiers, &hit.legend),
                ((footer.width - 20.0) / 6.0) as usize,
            ),
            10.0,
            FontWeight::Bold,
            palette.accent,
        );
        let text = actions
            .get(&hit.key)
            .map(|a| a.join(" · "))
            .unwrap_or_else(|| "No binding".into());
        for (i, line) in wrap_two_lines(&text, ((footer.width - 20.0) / 6.0) as usize)
            .iter()
            .enumerate()
        {
            draw_text(
                context,
                footer.x + 10.0,
                footer.y + 35.0 + i as f64 * 14.0,
                line,
                10.5,
                FontWeight::Normal,
                palette.foreground,
            );
        }
    } else {
        let count = controls
            .iter()
            .filter(|hit| actions.contains_key(&hit.key))
            .count();
        draw_text(
            context,
            footer.x + 10.0,
            footer.y + 23.0,
            &if count == 0 {
                "No bindings for this chord".into()
            } else {
                format!("{count} bound controls")
            },
            11.0,
            FontWeight::Bold,
            palette.foreground,
        );
        draw_text(
            context,
            footer.x + 10.0,
            footer.y + 43.0,
            "Hover a button or wheel",
            10.5,
            FontWeight::Normal,
            palette.dark_foreground,
        );
    }
    state.metrics.hits.extend(controls);
}

#[allow(clippy::too_many_arguments)]
fn draw_peripheral_panel(
    context: &Context,
    panel: Rect,
    title: &str,
    subtitle: &str,
    rows: &[PeripheralRow],
    scroll: &mut ScrollMetrics,
    row_hits: &mut Vec<(Rect, String)>,
    palette: Palette,
) {
    rounded_rectangle(context, panel, 16.0);
    set_source(context, palette.background.with_alpha(0.985));
    let _ = context.fill_preserve();
    context.set_line_width(1.0);
    set_source(context, palette.muted.with_alpha(0.9));
    let _ = context.stroke();
    draw_text(
        context,
        panel.x + 18.0,
        panel.y + 29.0,
        title,
        15.0,
        FontWeight::Bold,
        palette.foreground,
    );
    draw_text(
        context,
        panel.x + 18.0,
        panel.y + 49.0,
        &elide(subtitle, ((panel.width - 36.0) / 6.0) as usize),
        11.0,
        FontWeight::Normal,
        palette.dark_foreground,
    );
    let viewport = Rect {
        x: panel.x + 12.0,
        y: panel.y + 62.0,
        width: panel.width - 24.0,
        height: (panel.height - 72.0).max(0.0),
    };
    scroll.rect = panel;
    draw_scroll_rows(context, viewport, rows, scroll, row_hits, palette);
}

fn draw_scroll_rows(
    context: &Context,
    viewport: Rect,
    rows: &[PeripheralRow],
    scroll: &mut ScrollMetrics,
    row_hits: &mut Vec<(Rect, String)>,
    palette: Palette,
) {
    scroll.maximum = (rows.len() as f64 * 48.0 - viewport.height).max(0.0);
    scroll.offset = scroll.offset.clamp(0.0, scroll.maximum);
    let _ = context.save();
    context.rectangle(viewport.x, viewport.y, viewport.width, viewport.height);
    context.clip();
    for (index, row) in rows.iter().enumerate() {
        let rect = Rect {
            x: viewport.x,
            y: viewport.y + index as f64 * 48.0 - scroll.offset,
            width: viewport.width - 8.0,
            height: 42.0,
        };
        if rect.y + rect.height <= viewport.y || rect.y >= viewport.y + viewport.height {
            continue;
        }
        rounded_rectangle(context, rect, 7.0);
        set_source(context, palette.darker_background.with_alpha(0.58));
        let _ = context.fill();
        draw_text(
            context,
            rect.x + 10.0,
            rect.y + 15.0,
            &elide_to_width(
                context,
                &row.label,
                rect.width - 20.0,
                10.0,
                FontWeight::Bold,
            ),
            10.0,
            FontWeight::Bold,
            palette.accent,
        );
        draw_text(
            context,
            rect.x + 10.0,
            rect.y + 32.0,
            &elide_to_width(
                context,
                &row.action,
                rect.width - 20.0,
                10.5,
                FontWeight::Normal,
            ),
            10.5,
            FontWeight::Normal,
            palette.foreground,
        );
        let top = rect.y.max(viewport.y);
        let bottom = (rect.y + rect.height).min(viewport.y + viewport.height);
        row_hits.push((
            Rect {
                y: top,
                height: bottom - top,
                ..rect
            },
            format!("{}\n{}", row.label, row.action),
        ));
    }
    if scroll.maximum > 0.0 {
        let thumb_height = (viewport.height * viewport.height / (rows.len() as f64 * 48.0))
            .max(16.0)
            .min(viewport.height);
        let thumb = Rect {
            x: viewport.x + viewport.width - 4.0,
            y: viewport.y + (viewport.height - thumb_height) * scroll.offset / scroll.maximum,
            width: 3.0,
            height: thumb_height,
        };
        rounded_rectangle(context, thumb, 1.5);
        set_source(context, palette.accent.with_alpha(0.65));
        let _ = context.fill();
    }
    let _ = context.restore();
}

fn draw_header(context: &Context, state: &OverlayState, rect: Rect, palette: Palette) {
    let Rect {
        x,
        y,
        width,
        height,
    } = rect;
    let compact = height == 60.0;
    let title = if state.modifiers.is_empty() {
        "NO MODIFIERS".to_string()
    } else {
        state.modifiers.label()
    };
    draw_text(
        context,
        x,
        y + if compact { 18.0 } else { 24.0 },
        &elide_to_width(context, &title, width, 22.0, FontWeight::Bold),
        22.0,
        FontWeight::Bold,
        palette.foreground,
    );
    draw_text(
        context,
        x,
        y + if compact { 36.0 } else { 44.0 },
        &elide_to_width(
            context,
            &state.snapshot.device_name,
            width,
            13.0,
            FontWeight::Normal,
        ),
        13.0,
        FontWeight::Normal,
        palette.dark_foreground,
    );

    let message =
        state.snapshot.binding_error.as_deref().unwrap_or(
            "Hold any modifier, or click Shift, Ctrl, and Alt to keep them selected · Hover for details",
        );
    draw_text(
        context,
        x,
        y + if compact { 54.0 } else { 64.0 },
        &elide_to_width(context, message, width, 13.0, FontWeight::Normal),
        13.0,
        FontWeight::Normal,
        if state.snapshot.binding_error.is_some() {
            palette.red
        } else {
            palette.dark_foreground
        },
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_key(
    context: &Context,
    rect: Rect,
    legend: &str,
    actions: Option<&Vec<String>>,
    hovered: bool,
    modifier_active: bool,
    icon_atlas: Option<&IconAtlas>,
    palette: Palette,
) {
    rounded_rectangle(context, rect, (rect.height * 0.1).clamp(4.0, 8.0));
    let fill = if hovered {
        palette.accent.with_alpha(0.3)
    } else if actions.is_some() {
        palette.selection
    } else {
        palette.lighter_background.with_alpha(0.34)
    };
    set_source(context, fill);
    let _ = context.fill_preserve();
    context.set_line_width(if hovered || modifier_active { 2.0 } else { 1.0 });
    set_source(
        context,
        if hovered || modifier_active {
            palette.accent
        } else if actions.is_some() {
            palette.muted
        } else {
            palette.muted.with_alpha(0.42)
        },
    );
    let _ = context.stroke();

    let legend_color = if actions.is_some() || modifier_active {
        palette.foreground
    } else {
        palette.dark_foreground.with_alpha(0.66)
    };
    // Small full-size boards show fitted legends; actions remain available
    // in the details strip and searchable list.
    if rect.height < 24.0 {
        let size = (rect.height * 0.4).clamp(5.0, 8.0);
        let legend = elide_to_width(context, legend, rect.width - 4.0, size, FontWeight::Bold);
        draw_text_centered(
            context,
            rect.x + rect.width / 2.0,
            rect.y + (rect.height + size) / 2.0 - 1.0,
            &legend,
            size,
            FontWeight::Bold,
            legend_color,
        );
        return;
    }
    let legend_y = (rect.height * 0.32).clamp(9.0, 14.0);
    let legend_size = (rect.height * 0.16).clamp(8.0, 11.0);
    let legend = elide_to_width(
        context,
        legend,
        rect.width - 10.0,
        legend_size,
        FontWeight::Bold,
    );
    draw_text(
        context,
        rect.x + 5.0,
        rect.y + legend_y,
        &legend,
        legend_size,
        FontWeight::Bold,
        legend_color,
    );

    if let Some(actions) = actions {
        let content = Rect {
            x: rect.x + 4.0,
            y: rect.y + legend_y + 2.0,
            width: rect.width - 8.0,
            height: rect.height - legend_y - 4.0,
        };
        let _ = context.save();
        context.rectangle(content.x, content.y, content.width, content.height);
        context.clip();
        let icons = icons_for_actions(actions);
        if let (Some(atlas), Some(icons)) = (icon_atlas, icons) {
            draw_action_icons(context, atlas, content, &icons, palette.foreground);
        } else {
            let summary = actions.join(" · ");
            let font_size = (rect.height * 0.155).clamp(8.0, 11.5);
            let lines = if content.height >= font_size * 2.0 + 3.0 {
                wrap_two_lines(&summary, (content.width / 6.1).floor().max(1.0) as usize)
            } else {
                vec![summary]
            };
            let text_height = font_size + (lines.len() - 1) as f64 * (font_size + 3.0);
            let start_y = content.y + (content.height - text_height) / 2.0 + font_size;
            for (index, line) in lines.iter().enumerate() {
                draw_text_centered(
                    context,
                    rect.x + rect.width / 2.0,
                    start_y + index as f64 * (font_size + 3.0),
                    &elide_to_width(context, line, content.width, font_size, FontWeight::Normal),
                    font_size,
                    FontWeight::Normal,
                    palette.foreground,
                );
            }
        }
        let _ = context.restore();
    }
}

fn draw_action_icons(
    context: &Context,
    atlas: &IconAtlas,
    rect: Rect,
    icons: &[ActionIcon],
    color: Color,
) {
    if icons.is_empty() || rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }

    let gap = (rect.height * 0.08).clamp(2.0, 4.0);
    let available_per_icon =
        (rect.width - gap * icons.len().saturating_sub(1) as f64) / icons.len() as f64;
    let size = (rect.height * 0.92).min(available_per_icon).max(1.0);
    let total_width = size * icons.len() as f64 + gap * icons.len().saturating_sub(1) as f64;
    let start_x = rect.x + (rect.width - total_width) / 2.0;
    let y = rect.y + (rect.height - size) / 2.0;

    for (index, icon) in icons.iter().enumerate() {
        atlas.draw(
            context,
            *icon,
            start_x + index as f64 * (size + gap),
            y,
            size,
            color,
        );
    }
}

fn draw_details(
    context: &Context,
    state: &OverlayState,
    actions: &BTreeMap<String, Vec<String>>,
    rect: Rect,
    palette: Palette,
) {
    rounded_rectangle(context, rect, 9.0);
    set_source(context, palette.darker_background.with_alpha(0.72));
    let _ = context.fill();

    let second_line = (rect.height - 8.0).min(42.0);
    let first_line = (second_line - 18.0).min(21.0);
    let hovered = state.hovered.as_ref().and_then(|key| {
        state
            .metrics
            .hits
            .iter()
            .find(|hit| &hit.key == key)
            .map(|hit| (hit, actions.get(key)))
    });

    match hovered {
        Some((hit, Some(key_actions))) => {
            let chord = chord_label(state.modifiers, &hit.legend);
            draw_text(
                context,
                rect.x + 14.0,
                rect.y + first_line,
                &elide_to_width(context, &chord, rect.width - 28.0, 12.0, FontWeight::Bold),
                12.0,
                FontWeight::Bold,
                palette.accent,
            );
            draw_text(
                context,
                rect.x + 14.0,
                rect.y + second_line,
                &elide_to_width(
                    context,
                    &key_actions.join("  •  "),
                    rect.width - 28.0,
                    14.0,
                    FontWeight::Normal,
                ),
                14.0,
                FontWeight::Normal,
                palette.foreground,
            );
        }
        Some((hit, None)) => {
            draw_text(
                context,
                rect.x + 14.0,
                rect.y + (rect.height + 13.0) / 2.0,
                &elide_to_width(
                    context,
                    &format!(
                        "{}  ·  No binding",
                        chord_label(state.modifiers, &hit.legend)
                    ),
                    rect.width - 28.0,
                    13.0,
                    FontWeight::Normal,
                ),
                13.0,
                FontWeight::Normal,
                palette.dark_foreground,
            );
        }
        None => {
            draw_text(
                context,
                rect.x + 14.0,
                rect.y + first_line,
                &elide_to_width(
                    context,
                    "Hover a key or mouse control for details",
                    rect.width - 28.0,
                    13.0,
                    FontWeight::Normal,
                ),
                13.0,
                FontWeight::Normal,
                palette.dark_foreground,
            );
            draw_text(
                context,
                rect.x + 14.0,
                rect.y + second_line,
                "Esc or backdrop to close",
                12.0,
                FontWeight::Normal,
                palette.dark_foreground,
            );
        }
    }
}

fn modifier_key_active(key: &str, modifiers: Modifiers) -> bool {
    match key {
        "SHIFTLEFT" | "SHIFTRIGHT" => modifiers.contains(Modifiers::SHIFT),
        "CTRLLEFT" | "CTRLRIGHT" => modifiers.contains(Modifiers::CTRL),
        "ALTLEFT" | "ALTRIGHT" => modifiers.contains(Modifiers::ALT),
        "SUPERLEFT" | "SUPERRIGHT" => modifiers.contains(Modifiers::SUPER),
        _ => false,
    }
}

fn modifier_for_key_id(key: &str) -> Option<Modifiers> {
    match key {
        "SHIFTLEFT" | "SHIFTRIGHT" => Some(Modifiers::SHIFT),
        "CTRLLEFT" | "CTRLRIGHT" => Some(Modifiers::CTRL),
        "ALTLEFT" | "ALTRIGHT" => Some(Modifiers::ALT),
        _ => None,
    }
}

fn friendly_key_name(key: &str) -> String {
    match key {
        "RETURN" => "Enter".into(),
        "ESCAPE" => "Esc".into(),
        "MOUSE_SCROLL_LEFT" => "Wheel left".into(),
        "MOUSE_SCROLL_RIGHT" => "Wheel right".into(),
        "MOUSE_DOWN" => "Wheel down".into(),
        "MOUSE_UP" => "Wheel up".into(),
        "MOUSE_LEFT" => "Left click".into(),
        "MOUSE_RIGHT" => "Right click".into(),
        "MOUSE_MIDDLE" => "Middle click".into(),
        value if value.starts_with("MOUSE:") => value[6..]
            .parse::<u16>()
            .ok()
            .and_then(|code| code.checked_sub(271))
            .map(|button| format!("Button {button}"))
            .unwrap_or_else(|| value.into()),
        value if value.starts_with("XF86") => value.trim_start_matches("XF86").into(),
        value => value.replace('_', " ").to_ascii_lowercase(),
    }
}

fn wrap_two_lines(text: &str, max_chars: usize) -> Vec<String> {
    if text.chars().count() <= max_chars {
        return vec![text.to_string()];
    }

    let words: Vec<_> = text.split_whitespace().collect();
    let mut first = String::new();
    let mut second = String::new();
    for word in words {
        let target = if first.is_empty() || first.len() + 1 + word.len() <= max_chars {
            &mut first
        } else {
            &mut second
        };
        if !target.is_empty() {
            target.push(' ');
        }
        target.push_str(word);
    }
    if second.chars().count() > max_chars {
        second = elide(&second, max_chars);
    }
    if first.chars().count() > max_chars {
        first = elide(&first, max_chars);
    }
    vec![first, second]
        .into_iter()
        .filter(|line| !line.is_empty())
        .collect()
}

fn elide(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(1);
    format!("{}…", text.chars().take(keep).collect::<String>())
}

fn rounded_rectangle(context: &Context, rect: Rect, radius: f64) {
    let radius = radius.min(rect.width / 2.0).min(rect.height / 2.0);
    context.new_sub_path();
    context.arc(
        rect.x + rect.width - radius,
        rect.y + radius,
        radius,
        -std::f64::consts::FRAC_PI_2,
        0.0,
    );
    context.arc(
        rect.x + rect.width - radius,
        rect.y + rect.height - radius,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    );
    context.arc(
        rect.x + radius,
        rect.y + rect.height - radius,
        radius,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    context.arc(
        rect.x + radius,
        rect.y + radius,
        radius,
        std::f64::consts::PI,
        std::f64::consts::PI * 1.5,
    );
    context.close_path();
}

fn set_source(context: &Context, color: Color) {
    context.set_source_rgba(color.red, color.green, color.blue, color.alpha);
}

fn elide_to_width(
    context: &Context,
    text: &str,
    width: f64,
    size: f64,
    weight: FontWeight,
) -> String {
    context.select_font_face("sans", FontSlant::Normal, weight);
    context.set_font_size(size);
    let fits = |text: &str| {
        context
            .text_extents(text)
            .is_ok_and(|extent| extent.x_advance() <= width)
    };
    if fits(text) {
        return text.into();
    }
    let mut shortened = text.to_string();
    while shortened.pop().is_some() {
        let candidate = format!("{shortened}…");
        if fits(&candidate) {
            return candidate;
        }
    }
    String::new()
}

fn draw_text(
    context: &Context,
    x: f64,
    baseline_y: f64,
    text: &str,
    size: f64,
    weight: FontWeight,
    color: Color,
) {
    context.select_font_face("sans", FontSlant::Normal, weight);
    context.set_font_size(size);
    set_source(context, color);
    context.move_to(x, baseline_y);
    let _ = context.show_text(text);
}

fn draw_text_centered(
    context: &Context,
    center_x: f64,
    baseline_y: f64,
    text: &str,
    size: f64,
    weight: FontWeight,
    color: Color,
) {
    context.select_font_face("sans", FontSlant::Normal, weight);
    context.set_font_size(size);
    let width = context
        .text_extents(text)
        .map(|extents| extents.width())
        .unwrap_or(0.0);
    draw_text(
        context,
        center_x - width / 2.0,
        baseline_y,
        text,
        size,
        weight,
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Binding;
    use gtk::cairo::{Format, ImageSurface};

    #[test]
    fn search_initially_includes_all_binding_kinds_and_deduplicates() {
        let bindings = crate::core::parse_bindings(
            "SUPER + RETURN → Terminal\nALT + RETURN → Other terminal\nSUPER + XF86AudioMute → Mute\nSUPER + mouse:272 → Move window\nA → Plain action\nSUPER + RETURN → Terminal\nSUPER + RETURN → Second action",
        );
        let rows = shortcut_rows(&bindings, Modifiers::default(), "");
        assert_eq!(rows.len(), 6);
        for description in [
            "Terminal",
            "Other terminal",
            "Mute",
            "Move window",
            "Plain action",
            "Second action",
        ] {
            assert!(rows.iter().any(|row| row.action == description));
        }
        assert_eq!(rows, shortcut_rows(&bindings, Modifiers::default(), "   "));
        let mut reversed = bindings.clone();
        reversed.reverse();
        assert_eq!(rows, shortcut_rows(&reversed, Modifiers::default(), ""));
        assert_eq!(
            rows.iter().filter(|row| row.action == "Terminal").count(),
            1
        );
    }

    #[test]
    fn search_combines_exact_modifier_filter_with_fuzzy_tokens() {
        let bindings = crate::core::parse_bindings(
            "SUPER + RETURN → Open terminal\nSUPER SHIFT + RETURN → Open terminal split\nALT + RETURN → Open terminal alternate\nA → Unmodified terminal\nSUPER + mouse:272 → Move window\nSUPER + XF86AudioMute → Silence sound",
        );
        assert_eq!(shortcut_rows(&bindings, Modifiers::SUPER, "trm").len(), 1);
        assert_eq!(
            shortcut_rows(&bindings, Modifiers::SUPER.with(Modifiers::SHIFT), "trm")[0].action,
            "Open terminal split"
        );
        assert_eq!(
            shortcut_rows(&bindings, Modifiers::default(), "trm").len(),
            4
        );
        for query in ["RETURN", "EnTeR", "sup trm", "super return term"] {
            assert_eq!(
                shortcut_rows(&bindings, Modifiers::SUPER, query)[0].action,
                "Open terminal"
            );
        }
        for query in ["mouse_left", "left click", "mv wnd"] {
            assert_eq!(
                shortcut_rows(&bindings, Modifiers::default(), query)[0].action,
                "Move window"
            );
        }
        assert_eq!(
            shortcut_rows(&bindings, Modifiers::default(), "audiomute")[0].action,
            "Silence sound"
        );
        assert!(shortcut_rows(&bindings, Modifiers::CTRL, "").is_empty());
        assert!(shortcut_rows(&bindings, Modifiers::default(), "zzzz").is_empty());
        assert!(shortcut_rows(&bindings, Modifiers::SUPER, "term nonexistent").is_empty());
    }

    #[test]
    fn search_ranks_exact_then_contiguous_then_scattered_matches() {
        let bindings = crate::core::parse_bindings(
            "SUPER + A → Terminal\nSUPER + B → Open term\nSUPER + C → Term\nSUPER + D → The emergency room\nSUPER + E → Term\nSUPER + F → Über öffnen",
        );
        let rows = shortcut_rows(&bindings, Modifiers::default(), "term");
        assert_eq!(
            rows.iter()
                .map(|row| row.action.as_str())
                .collect::<Vec<_>>(),
            [
                "Term",
                "Term",
                "Terminal",
                "Open term",
                "The emergency room"
            ]
        );
        assert!(rows[0].label < rows[1].label);
        assert_eq!(
            shortcut_rows(&bindings, Modifiers::default(), "ÜBR ÖFN")[0].action,
            "Über öffnen"
        );
    }

    #[test]
    fn search_scrolls_independently_and_filtering_keeps_panels_stable() {
        let surface = ImageSurface::create(Format::ARgb32, 1280, 720).unwrap();
        let context = Context::new(&surface).unwrap();
        let mut state = fixture_state(KeyboardLayout::Tkl, true, true);
        for index in 0..100 {
            state.snapshot.bindings.push(Binding {
                modifiers: Modifiers::SUPER,
                key: "F24".into(),
                action: format!("Extra action {index:03}"),
            });
        }
        draw_overlay(&context, 1280.0, 720.0, &mut state);
        let panels = state.metrics.panels.clone();
        let search = state.metrics.search.rect;
        state.metrics.pointer = Some((search.x + 5.0, search.y + 5.0));
        assert!(state.metrics.contains_panel(search.x, search.y));
        assert!(state.metrics.scroll(100000.0));
        assert_eq!(state.metrics.search.offset, state.metrics.search.maximum);
        assert!(state.metrics.search.offset > 0.0);
        assert_eq!(state.metrics.touchpad.offset, 0.0);
        draw_overlay(&context, 1280.0, 720.0, &mut state);
        assert!(
            state
                .metrics
                .rows
                .iter()
                .filter(|(rect, _)| rect.x == search.x)
                .all(|(rect, _)| rect.y >= search.y
                    && rect.y + rect.height <= search.y + search.height)
        );
        let before = state.metrics.search.offset;
        let pad = state.metrics.touchpad.rect;
        state.metrics.pointer = Some((pad.x + 5.0, pad.y + 5.0));
        assert!(state.metrics.scroll(100000.0));
        assert_eq!(state.metrics.search.offset, before);
        state.query = "no such shortcut".into();
        state.modifiers = Modifiers::ALT;
        draw_overlay(&context, 1280.0, 720.0, &mut state);
        assert_eq!(state.metrics.panels, panels);
        assert_eq!(state.metrics.search.maximum, 0.0);
        assert_eq!(state.metrics.search.offset, 0.0);
        assert!(
            !state
                .metrics
                .rows
                .iter()
                .any(|(rect, _)| rect.x == search.x)
        );
    }

    #[test]
    fn maps_only_the_current_physical_gdk_modifier_state() {
        assert_eq!(
            modifiers_from_gdk(gdk::ModifierType::empty()),
            Modifiers::default()
        );
        assert_eq!(
            modifiers_from_gdk(
                gdk::ModifierType::SHIFT_MASK
                    | gdk::ModifierType::CONTROL_MASK
                    | gdk::ModifierType::ALT_MASK,
            ),
            Modifiers::SHIFT.with(Modifiers::CTRL).with(Modifiers::ALT)
        );
        assert_eq!(
            modifiers_from_gdk(gdk::ModifierType::SUPER_MASK),
            Modifiers::default()
        );
    }

    #[test]
    fn state_changes_prefer_post_event_device_mask_over_stale_signal_mask() {
        assert_eq!(
            modifiers_from_state_change(
                gdk::ModifierType::SHIFT_MASK | gdk::ModifierType::ALT_MASK,
                Some(gdk::ModifierType::ALT_MASK),
                Modifiers::default(),
            ),
            Modifiers::ALT
        );
        assert_eq!(
            modifiers_from_state_change(
                gdk::ModifierType::ALT_MASK,
                Some(gdk::ModifierType::empty()),
                Modifiers::default(),
            ),
            Modifiers::default()
        );
        assert_eq!(
            modifiers_from_state_change(gdk::ModifierType::ALT_MASK, None, Modifiers::default()),
            Modifiers::ALT
        );
    }

    #[test]
    fn startup_super_state_is_ignored_until_a_fresh_key_press() {
        assert_eq!(
            modifiers_from_state_change(
                gdk::ModifierType::SUPER_MASK,
                Some(gdk::ModifierType::SUPER_MASK),
                Modifiers::default(),
            ),
            Modifiers::default()
        );
        assert_eq!(
            modifiers_from_state_change(
                gdk::ModifierType::SUPER_MASK,
                Some(gdk::ModifierType::SUPER_MASK),
                Modifiers::SUPER,
            ),
            Modifiers::SUPER
        );
        assert_eq!(
            modifiers_from_state_change(
                gdk::ModifierType::SUPER_MASK,
                Some(gdk::ModifierType::empty()),
                Modifiers::SUPER,
            ),
            Modifiers::default()
        );
    }

    #[test]
    fn modifier_key_events_only_change_their_mapped_modifier() {
        for (key, modifier) in [
            (gdk::Key::Shift_L, Modifiers::SHIFT),
            (gdk::Key::Shift_R, Modifiers::SHIFT),
            (gdk::Key::Control_L, Modifiers::CTRL),
            (gdk::Key::Control_R, Modifiers::CTRL),
            (gdk::Key::Alt_L, Modifiers::ALT),
            (gdk::Key::Alt_R, Modifiers::ALT),
            (gdk::Key::Super_L, Modifiers::SUPER),
            (gdk::Key::Super_R, Modifiers::SUPER),
        ] {
            assert_eq!(
                modifiers_for_key_event(Modifiers::default(), key, true),
                Some(modifier)
            );
            assert_eq!(
                modifiers_for_key_event(modifier, key, false),
                Some(Modifiers::default())
            );
        }

        assert_eq!(
            modifiers_for_key_event(
                Modifiers::SUPER.with(Modifiers::ALT),
                gdk::Key::VoidSymbol,
                false,
            ),
            None
        );
    }

    #[test]
    fn super_can_be_held_but_not_click_latched() {
        assert_eq!(modifier_for_key_id("SUPERLEFT"), None);
        assert_eq!(modifier_for_key_id("SUPERRIGHT"), None);
        assert_eq!(modifier_for_key_id("SHIFTLEFT"), Some(Modifiers::SHIFT));
        assert!(!modifier_key_active("SUPERLEFT", Modifiers::default()));
        assert!(modifier_key_active("SUPERLEFT", Modifiers::SUPER));
    }

    #[test]
    fn mouse_diagram_follows_exact_modifiers_and_keeps_geometry_stable() {
        let surface = ImageSurface::create(Format::ARgb32, 1920, 1080).unwrap();
        let context = Context::new(&surface).unwrap();
        let mut state = fixture_state(KeyboardLayout::Tkl, false, false);
        state.snapshot.bindings.push(Binding {
            modifiers: Modifiers::default(),
            key: "MOUSE_MIDDLE".into(),
            action: "Middle action".into(),
        });
        draw_overlay(&context, 1920.0, 1080.0, &mut state);
        let panel = state.metrics.mouse;
        let regions: Vec<_> = state
            .metrics
            .hits
            .iter()
            .filter(|hit| is_mouse_key(&hit.key))
            .map(|hit| (hit.key.clone(), hit.rect))
            .collect();
        assert_eq!(regions.len(), 5);
        let wheel = state
            .metrics
            .hits
            .iter()
            .find(|hit| hit.key == "MOUSE_UP")
            .unwrap()
            .clone();
        let keys = state
            .snapshot
            .bindings
            .iter()
            .filter(|b| is_mouse_key(&b.key))
            .map(|b| b.key.clone())
            .collect();
        let mut images = Vec::new();
        for (modifiers, expected) in [
            (Modifiers::SUPER, "Previous workspace"),
            (
                Modifiers::SUPER.with(Modifiers::ALT),
                "Previous group window",
            ),
            (Modifiers::ALT, "No binding"),
            (Modifiers::default(), "No binding"),
        ] {
            state.modifiers = modifiers;
            let actions = actions_for(&state.snapshot.bindings, modifiers);
            assert!(mouse_tooltip(&wheel, &actions, modifiers).ends_with(expected));
            let mouse_surface = ImageSurface::create(Format::ARgb32, 1920, 1080).unwrap();
            let mouse_context = Context::new(&mouse_surface).unwrap();
            draw_mouse_panel(&mouse_context, panel, &keys, &actions, &mut state);
            let mut png = Vec::new();
            mouse_surface.write_to_png(&mut png).unwrap();
            images.push(png);
            draw_overlay(&context, 1920.0, 1080.0, &mut state);
            assert_eq!(panel, state.metrics.mouse);
            assert_eq!(
                regions,
                state
                    .metrics
                    .hits
                    .iter()
                    .filter(|hit| is_mouse_key(&hit.key))
                    .map(|hit| (hit.key.clone(), hit.rect))
                    .collect::<Vec<_>>()
            );
        }
        assert!(images.windows(2).all(|pair| pair[0] != pair[1]));
        let middle = state
            .metrics
            .hits
            .iter()
            .find(|hit| hit.key == "MOUSE_MIDDLE")
            .unwrap();
        assert!(
            mouse_tooltip(
                middle,
                &actions_for(&state.snapshot.bindings, Modifiers::default()),
                Modifiers::default()
            )
            .ends_with("Middle action")
        );
    }

    #[test]
    fn mouse_geometry_preserves_optional_controls_on_both_sides() {
        let keys = [
            "MOUSE_SCROLL_LEFT",
            "MOUSE_SCROLL_RIGHT",
            "MOUSE:275",
            "MOUSE:276",
            "MOUSE:277",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        for left in [false, true] {
            let layout = panel_layout(
                1024.0,
                600.0,
                &KeyboardGeometry::new(KeyboardLayout::Full),
                true,
                10,
                left,
            );
            let panel = layout.mouse.unwrap();
            let (_, controls) = mouse_geometry(panel, &keys);
            for key in &keys {
                assert!(controls.iter().any(|hit| &hit.key == key));
            }
            for (index, a) in controls.iter().enumerate() {
                assert!(
                    panel.contains(a.rect.x, a.rect.y)
                        && panel.contains(a.rect.x + a.rect.width, a.rect.y + a.rect.height)
                );
                for b in controls.iter().skip(index + 1) {
                    assert!(
                        a.rect.x + a.rect.width <= b.rect.x
                            || b.rect.x + b.rect.width <= a.rect.x
                            || a.rect.y + a.rect.height <= b.rect.y
                            || b.rect.y + b.rect.height <= a.rect.y
                    );
                }
            }
        }
    }

    fn fixture_state(layout: KeyboardLayout, left: bool, touchpad: bool) -> OverlayState {
        use crate::peripherals::{
            DiscoveryStatus, GestureBinding, GestureDiscovery, PointingDevices, Touchpad,
        };
        let bindings = crate::core::parse_bindings(
            "SUPER + RETURN → Terminal\nSUPER + B → Browser\nSUPER + Q → Close window\nSUPER + mouse:272 → Move window\nSUPER + mouse:273 → Resize window\nSUPER + mouse_up → Previous workspace\nSUPER + mouse_down → Next workspace\nSUPER ALT + mouse_up → Previous group window\nSUPER ALT + mouse_down → Next group window",
        );
        let pointing = PointingDevices {
            mice: vec!["Example mouse".into()],
            mouse_left: left,
            touchpads: if touchpad {
                vec![Touchpad {
                    name: "Example precision touchpad".into(),
                    settings: BTreeMap::from([
                        ("tap_to_click".into(), "true".into()),
                        ("tap_button_map".into(), "lrm".into()),
                        ("clickfinger_behavior".into(), "true".into()),
                        ("natural_scroll".into(), "true".into()),
                        ("scroll_method".into(), "2fg".into()),
                    ]),
                }]
            } else {
                Vec::new()
            },
            gestures: GestureDiscovery {
                status: DiscoveryStatus::Complete,
                bindings: vec![GestureBinding {
                    fingers: 3,
                    direction: "horizontal".into(),
                    modifiers: Modifiers::default(),
                    action: "Switch workspaces".into(),
                    scale: "1".into(),
                }],
            },
        };
        OverlayState {
            search_style: SearchStyle::default(),
            native_entry: false,
            snapshot: RuntimeSnapshot {
                bindings,
                binding_error: None,
                layout,
                device_name: format!("Example keyboard · {}", layout.label()),
                monitor_connector: None,
                palette: Palette::default(),
                pointing,
            },
            geometry: KeyboardGeometry::new(layout),
            latched_modifiers: Modifiers::default(),
            held_modifiers: Modifiers::SUPER,
            modifiers: Modifiers::SUPER,
            query: String::new(),
            hovered: None,
            metrics: RenderMetrics::default(),
        }
    }

    fn assert_contains(outer: Rect, inner: Rect) {
        let epsilon = 0.001;
        assert!(inner.width > 0.0 && inner.height > 0.0, "{inner:?}");
        assert!(
            inner.x >= outer.x - epsilon
                && inner.y >= outer.y - epsilon
                && inner.x + inner.width <= outer.x + outer.width + epsilon
                && inner.y + inner.height <= outer.y + outer.height + epsilon,
            "{inner:?} outside {outer:?}"
        );
    }

    fn assert_separated(a: Rect, b: Rect) {
        let epsilon = 0.001;
        assert!(
            a.x + a.width + 24.0 <= b.x + epsilon
                || b.x + b.width + 24.0 <= a.x + epsilon
                || a.y + a.height + 24.0 <= b.y + epsilon
                || b.y + b.height + 24.0 <= a.y + epsilon,
            "{a:?} overlaps {b:?}"
        );
    }

    #[test]
    fn panel_layout_keeps_all_devices_on_screen_and_gaps_outside_hit_regions() {
        let keys = [
            "MOUSE_SCROLL_LEFT",
            "MOUSE_SCROLL_RIGHT",
            "MOUSE:275",
            "MOUSE:276",
            "MOUSE:277",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        for keyboard in [
            KeyboardLayout::Full,
            KeyboardLayout::Tkl,
            KeyboardLayout::Compact75,
            KeyboardLayout::Compact65,
            KeyboardLayout::Compact60,
        ] {
            let geometry = KeyboardGeometry::new(keyboard);
            for (width, height) in [
                (1280.0, 720.0),
                (1920.0, 1080.0),
                (2560.0, 1440.0),
                (720.0, 1280.0),
                (1080.0, 1920.0),
                (1000.0, 1000.0),
                (720.0, 720.0),
                (1024.0, 600.0),
                (600.0, 1024.0),
            ] {
                for left in [false, true] {
                    for show_mouse in [false, true] {
                        for touch in [0, 1, 20, 100] {
                            let layout =
                                panel_layout(width, height, &geometry, show_mouse, touch, left);
                            let mut panels = vec![layout.keyboard, layout.search];
                            panels.extend(layout.mouse);
                            panels.extend(layout.touchpad);
                            let screen = Rect {
                                x: 0.0,
                                y: 0.0,
                                width,
                                height,
                            };
                            for (i, panel) in panels.iter().enumerate() {
                                assert_contains(screen, *panel);
                                for other in panels.iter().skip(i + 1) {
                                    assert_separated(*panel, *other);
                                }
                            }
                            assert!(layout.unit > 0.0 && layout.unit <= 78.0);
                            assert_contains(
                                layout.search,
                                search_entry_rect(layout.search, &SearchStyle::default()),
                            );
                            assert!(layout.search.height > 124.0);
                            for hit in layout.keyboard_hits(&geometry) {
                                assert_contains(layout.keyboard, hit.rect);
                            }
                            assert_eq!(layout.mouse.is_some(), show_mouse);
                            assert_eq!(layout.touchpad.is_some(), touch > 0);
                            if let Some(mouse) = layout.mouse {
                                let (body, hits) = mouse_geometry(mouse, &keys);
                                assert_contains(mouse, body);
                                for hit in hits {
                                    assert_contains(mouse, hit.rect);
                                }
                                if layout.mode == LayoutMode::Wide {
                                    assert_eq!(mouse.x < layout.keyboard.x, left);
                                } else if layout.mode == LayoutMode::Portrait {
                                    assert_eq!(mouse.x < layout.search.x, left);
                                } else if let Some(pad) = layout.touchpad {
                                    assert_eq!(mouse.x < pad.x, left);
                                }
                            }
                            if let Some(pad) = layout.touchpad {
                                assert!(pad.height > 72.0);
                            }
                            let metrics = RenderMetrics {
                                panels,
                                ..Default::default()
                            };
                            for panel in &metrics.panels {
                                assert!(metrics.contains_panel(panel.x + 1.0, panel.y + 1.0));
                                assert!(
                                    !metrics.contains_panel(
                                        panel.x + panel.width / 2.0,
                                        panel.y - 12.0
                                    )
                                );
                            }
                            if layout.mode != LayoutMode::Wide {
                                assert!(
                                    layout.search.y
                                        >= layout.keyboard.y + layout.keyboard.height + 24.0
                                            - 0.001
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn layout_switches_at_orientation_and_readability_boundaries() {
        let geometry = KeyboardGeometry::new(KeyboardLayout::Full);
        for (width, height) in [(999.0, 1000.0), (1000.0, 1000.0), (1001.0, 1000.0)] {
            let layout = panel_layout(width, height, &geometry, true, 20, false);
            assert_eq!(layout.mode == LayoutMode::Portrait, height > width);
        }
        // Locate the width threshold without duplicating the layout arithmetic.
        let width = (1280..1920)
            .find(|width| {
                wide_panel_layout(*width as f64, 1080.0, &geometry, true, 20, false).unit >= 32.0
            })
            .unwrap() as f64;
        let (mut low, mut high) = (width - 1.0, width);
        for _ in 0..40 {
            let middle = (low + high) / 2.0;
            if wide_panel_layout(middle, 1080.0, &geometry, true, 20, false).unit < 32.0 {
                low = middle;
            } else {
                high = middle;
            }
        }
        for (width, mode) in [(low, LayoutMode::Compact), (high, LayoutMode::Wide)] {
            assert_eq!(
                panel_layout(width, 1080.0, &geometry, true, 20, false).mode,
                mode
            );
        }
        let wide = wide_panel_layout(1280.0, 720.0, &geometry, true, 20, false);
        let compact = panel_layout(1280.0, 720.0, &geometry, true, 20, false);
        assert_eq!(compact.mode, LayoutMode::Compact);
        assert!(compact.unit >= 32.0 && compact.unit > wide.unit);
        assert_eq!(compact.chrome(), (16.0, 60.0, 48.0));
        assert!(compact.search.height >= 300.0 - 0.001);
    }

    #[test]
    fn resizing_preserves_filters_and_refreshes_hover_and_scroll_geometry() {
        let surface = ImageSurface::create(Format::ARgb32, 1920, 1920).unwrap();
        let context = Context::new(&surface).unwrap();
        let mut state = fixture_state(KeyboardLayout::Full, false, true);
        for index in 0..100 {
            state.snapshot.bindings.push(Binding {
                modifiers: Modifiers::SUPER,
                key: "MOUSE_LEFT".into(),
                action: format!("Extra action {index:03}"),
            });
        }
        state.query = "Extra".into();
        state.held_modifiers = Modifiers::SUPER;
        draw_overlay(&context, 1920.0, 1080.0, &mut state);
        let hit = state
            .metrics
            .hits
            .iter()
            .find(|hit| hit.key == "MOUSE_LEFT")
            .unwrap();
        state.metrics.pointer = Some((hit.rect.x + 1.0, hit.rect.y + 1.0));
        state.hovered = Some(hit.key.clone());
        assert!(tooltip_at_pointer(&state).is_some());
        state.metrics.search.offset = state.metrics.search.maximum;
        state.metrics.touchpad.offset = state.metrics.touchpad.maximum;
        for (width, height) in [
            (720.0, 1280.0),
            (1280.0, 720.0),
            (1080.0, 1920.0),
            (1920.0, 1080.0),
        ] {
            draw_overlay(&context, width, height, &mut state);
            assert_eq!(state.query, "Extra");
            assert_eq!(state.modifiers, Modifiers::SUPER);
            assert_eq!(state.held_modifiers, Modifiers::SUPER);
            let (x, y) = state.metrics.pointer.unwrap();
            assert_eq!(
                state.hovered,
                state
                    .metrics
                    .hits
                    .iter()
                    .find(|hit| hit.rect.contains(x, y))
                    .map(|hit| hit.key.clone())
            );
            for scroll in [&state.metrics.search, &state.metrics.touchpad] {
                assert!(scroll.offset >= 0.0 && scroll.offset <= scroll.maximum);
            }
            for (rect, _) in &state.metrics.rows {
                let viewport = if state.metrics.search.rect.contains(rect.x, rect.y) {
                    state.metrics.search.rect
                } else {
                    state.metrics.touchpad.rect
                };
                assert_contains(viewport, *rect);
            }
            let search = state.metrics.search.rect;
            state.metrics.pointer = Some((search.x + 5.0, search.y + 5.0));
            let touch_offset = state.metrics.touchpad.offset;
            assert!(state.metrics.scroll(-1.0));
            assert_eq!(state.metrics.touchpad.offset, touch_offset);
        }
    }

    #[test]
    fn touchpad_scrolling_preserves_mouse_diagram() {
        let surface = ImageSurface::create(Format::ARgb32, 1280, 720).unwrap();
        let context = Context::new(&surface).unwrap();
        let mut state = fixture_state(KeyboardLayout::Tkl, false, true);
        for i in 0..100 {
            state.snapshot.bindings.push(Binding {
                modifiers: Modifiers::SUPER,
                key: "MOUSE_LEFT".into(),
                action: format!("Action {i:03}"),
            });
        }
        draw_overlay(&context, 1280.0, 720.0, &mut state);
        let mouse = state.metrics.mouse;
        state.metrics.pointer = Some((mouse.x + 10.0, mouse.y + 10.0));
        assert!(!state.metrics.scroll(100000.0));
        assert_eq!(state.metrics.touchpad.offset, 0.0);
        let pad = state.metrics.touchpad.rect;
        state.metrics.pointer = Some((pad.x + 10.0, pad.y + 10.0));
        state.metrics.scroll(100000.0);
        draw_overlay(&context, 1280.0, 720.0, &mut state);
        assert!(
            state
                .metrics
                .rows
                .iter()
                .any(|(_, text)| text.contains("Two-finger scrolling"))
        );
        state.modifiers = Modifiers::ALT;
        let before = state.metrics.touchpad.offset;
        draw_overlay(&context, 1280.0, 720.0, &mut state);
        assert_eq!(state.metrics.touchpad.offset, before);
        state.metrics.scroll(-100000.0);
        assert_eq!(state.metrics.touchpad.offset, 0.0);
    }

    #[test]
    fn render_panel_fixtures() {
        // Set KEYWRLD_RENDER_DIR to retain the Cairo previews for visual QA.
        let directory = std::env::var_os("KEYWRLD_RENDER_DIR").map(std::path::PathBuf::from);
        if let Some(path) = &directory {
            std::fs::create_dir_all(path).unwrap();
        }
        for (name, layout, width, height, left, touch) in [
            ("full", KeyboardLayout::Full, 1920, 1080, false, true),
            ("full-compact", KeyboardLayout::Full, 1280, 720, false, true),
            ("portrait", KeyboardLayout::Full, 720, 1280, false, true),
            ("portrait-left", KeyboardLayout::Tkl, 1080, 1920, true, true),
            (
                "portrait-search",
                KeyboardLayout::Compact60,
                720,
                1280,
                false,
                false,
            ),
            ("square", KeyboardLayout::Full, 1000, 1000, false, true),
            ("long-labels", KeyboardLayout::Full, 720, 1280, true, true),
            ("tkl-left", KeyboardLayout::Tkl, 1920, 1080, true, true),
            ("75", KeyboardLayout::Compact75, 1280, 720, false, true),
            ("65", KeyboardLayout::Compact65, 1280, 720, true, true),
            (
                "60-small",
                KeyboardLayout::Compact60,
                1024,
                600,
                false,
                true,
            ),
            ("mouse-only", KeyboardLayout::Tkl, 1920, 1080, false, false),
            (
                "full-small-left",
                KeyboardLayout::Full,
                1024,
                600,
                true,
                true,
            ),
            ("search-all", KeyboardLayout::Tkl, 1280, 720, false, false),
            ("search-query", KeyboardLayout::Tkl, 1280, 720, false, false),
            ("search-empty", KeyboardLayout::Tkl, 1280, 720, false, false),
            (
                "search-theme-dark",
                KeyboardLayout::Tkl,
                1280,
                720,
                false,
                false,
            ),
            (
                "search-theme-light",
                KeyboardLayout::Tkl,
                720,
                1280,
                false,
                false,
            ),
            (
                "search-theme-large",
                KeyboardLayout::Tkl,
                1280,
                720,
                false,
                false,
            ),
        ] {
            let surface = ImageSurface::create(Format::ARgb32, width, height).unwrap();
            let context = Context::new(&surface).unwrap();
            let mut state = fixture_state(layout, left, touch);
            if name.starts_with("search-theme-") {
                let style = &mut state.search_style;
                style.font = "monospace".into();
                style.radius = 0.0;
                style.border_width = 2.0;
                style.foreground = Color::rgb(169, 177, 214);
                style.controls[2] = ControlStyle {
                    fill: style.foreground.with_alpha(0.08),
                    border: style.foreground.with_alpha(0.25),
                    width: 1.0,
                };
                if name == "search-theme-light" {
                    style.background = Color::rgb(245, 245, 240);
                    style.foreground = Color::rgb(40, 40, 48);
                    style.secondary = Color::rgb(85, 85, 90);
                    style.accent = Color::rgb(45, 70, 150);
                    style.border = style.accent;
                    style.radius = 8.0;
                    style.controls[2] = ControlStyle {
                        fill: style.foreground.with_alpha(0.08),
                        border: style.foreground.with_alpha(0.25),
                        width: 1.0,
                    };
                    style.row_hover.fill = style.foreground.with_alpha(0.08);
                    style.row_hover_text = style.accent;
                }
                if name == "search-theme-large" {
                    style.body = 20.0;
                    style.caption = 16.0;
                    style.heading = 26.0;
                    style.padding_y = 12.0;
                    state.snapshot.bindings[0].action = "An unusually long action description that must fit using actual font metrics".into();
                }
            }
            if name == "long-labels" {
                state.snapshot.device_name =
                    "A keyboard with an unusually long manufacturer and model name".repeat(3);
                for index in 0..100 {
                    state.snapshot.bindings.push(Binding { modifiers: Modifiers::SUPER,
                        key: "MOUSE_LEFT".into(), action: format!("{index}: A very long action containing WWWWW and unicode 漢字 to exercise fitting") });
                }
            }
            if name == "portrait-search" {
                state.snapshot.pointing.mice.clear();
                state
                    .snapshot
                    .bindings
                    .retain(|binding| !is_mouse_key(&binding.key));
            }
            if name.starts_with("search-") {
                state.modifiers = Modifiers::default();
                state.query = match name {
                    "search-query" => "trm",
                    "search-empty" => "zzzz",
                    _ => "",
                }
                .into();
                state.snapshot.bindings.push(Binding {
                    modifiers: Modifiers::ALT,
                    key: "F24".into(),
                    action: "Unmatched shortcut".into(),
                });
            }
            draw_overlay(&context, width as f64, height as f64, &mut state);
            if name.starts_with("search-theme-") {
                let viewport = state.metrics.search.rect;
                state.metrics.pointer = Some((viewport.x + 10.0, viewport.y + 10.0));
                state.metrics.search.offset = state.metrics.search.maximum / 2.0;
                draw_overlay(&context, width as f64, height as f64, &mut state);
                let layout = overlay_layout(width as f64, height as f64, &state);
                assert_contains(
                    layout.search,
                    search_entry_rect(layout.search, &state.search_style),
                );
                assert_contains(layout.search, state.metrics.search.rect);
            }
            assert_eq!(
                state.metrics.panels.len(),
                if touch {
                    4
                } else if name == "portrait-search" {
                    2
                } else {
                    3
                }
            );
            if name == "mouse-only" {
                for (label, modifiers) in [
                    ("none", Modifiers::default()),
                    ("super-alt", Modifiers::SUPER.with(Modifiers::ALT)),
                    ("hover-wheel", Modifiers::SUPER),
                ] {
                    state.modifiers = modifiers;
                    state.hovered = (label == "hover-wheel").then(|| "MOUSE_UP".into());
                    draw_overlay(&context, width as f64, height as f64, &mut state);
                    if let Some(path) = &directory {
                        surface
                            .write_to_png(
                                &mut std::fs::File::create(path.join(format!("mouse-{label}.png")))
                                    .unwrap(),
                            )
                            .unwrap();
                    }
                }
                state.hovered = None;
                state.modifiers = Modifiers::SUPER;
                draw_overlay(&context, width as f64, height as f64, &mut state);
            }
            if let Some(path) = &directory {
                surface
                    .write_to_png(
                        &mut std::fs::File::create(path.join(format!("{name}.png"))).unwrap(),
                    )
                    .unwrap();
            }
        }
    }
    #[test]
    #[ignore = "requires a graphical session; allocates an unmapped GTK window"]
    fn native_search_entry_tracks_responsive_layout() {
        gtk::init().unwrap();
        let application = gtk::Application::builder()
            .application_id("dev.keywrld.LayoutTest")
            .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
            .build();
        application
            .register(None::<&gtk::gio::Cancellable>)
            .unwrap();
        let state = fixture_state(KeyboardLayout::Full, false, true);
        let probe_window = gtk::Window::new();
        probe_window.add_css_class("keywrld-overlay");
        let probe = gtk::Entry::new();
        probe.add_css_class("shortcut-search");
        probe_window.set_child(Some(&probe));
        let baseline_color = probe.style_context().color();
        let search_style = SearchStyle::load(&gtk::Entry::new());
        let css_errors = Rc::new(RefCell::new(Vec::new()));
        let provider = gtk::CssProvider::new();
        provider.connect_parsing_error({
            let errors = css_errors.clone();
            move |_, _, error| errors.borrow_mut().push(error.to_string())
        });
        provider.load_from_data(&search_style.css());
        assert!(css_errors.borrow().is_empty(), "{:?}", css_errors.borrow());
        let window = build_window(&application, state.snapshot.clone());
        gtk::prelude::WidgetExt::realize(&window);
        let overlay = window.child().unwrap().downcast::<gtk::Overlay>().unwrap();
        let entry = overlay
            .last_child()
            .unwrap()
            .downcast::<gtk::Entry>()
            .unwrap();
        entry.set_text("preserved query");
        if let Ok(scale) = std::env::var("GDK_SCALE") {
            assert_eq!(entry.scale_factor(), scale.parse::<i32>().unwrap());
        }
        for (width, height) in [(1280, 720), (720, 1280), (1920, 1080), (1080, 1920)] {
            overlay.allocate(width, height, -1, None);
            let expected = search_entry_rect(
                overlay_layout(width as f64, height as f64, &state).search,
                &search_style,
            );
            let bounds = entry.compute_bounds(&overlay).unwrap();
            for (actual, expected) in [
                (bounds.x() as f64, expected.x),
                (bounds.y() as f64, expected.y),
                (bounds.width() as f64, expected.width),
                (bounds.height() as f64, expected.height),
            ] {
                assert!(
                    (actual - expected.round()).abs() <= 1.0,
                    "{bounds:?} != {expected}"
                );
            }
            assert_eq!(entry.text().as_str(), "preserved query");
        }
        if let Some(directory) = std::env::var_os("KEYWRLD_RENDER_DIR") {
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory).unwrap();
            // GTK only snapshots mapped widgets. Preview mode briefly maps the
            // test overlay without taking keyboard focus, then closes it below.
            window.set_keyboard_mode(KeyboardMode::None);
            window.present();
            let main = gtk::glib::MainContext::default();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            while !entry.is_mapped() && std::time::Instant::now() < deadline {
                while main.pending() {
                    main.iteration(false);
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            for (name, flags, value) in [
                ("placeholder", gtk::StateFlags::NORMAL, ""),
                ("hover", gtk::StateFlags::PRELIGHT, "Search shortcuts"),
                ("focus", gtk::StateFlags::FOCUS_WITHIN, "Search shortcuts"),
                (
                    "selection",
                    gtk::StateFlags::FOCUS_WITHIN,
                    "Search shortcuts",
                ),
            ] {
                entry.set_text(value);
                entry.set_state_flags(flags, true);
                if name == "selection" {
                    entry.select_region(0, -1);
                }
                while main.pending() {
                    main.iteration(false);
                }
                overlay.allocate(1280, 720, -1, None);
                let snapshot = gtk::Snapshot::new();
                overlay.snapshot_child(&entry, &snapshot);
                let node = snapshot.to_node().expect("native entry must render");
                let bounds = entry.compute_bounds(&overlay).unwrap();
                let surface = ImageSurface::create(
                    Format::ARgb32,
                    bounds.width().ceil() as i32,
                    bounds.height().ceil() as i32,
                )
                .unwrap();
                let context = Context::new(&surface).unwrap();
                set_source(&context, search_style.background);
                context.paint().unwrap();
                context.translate(-bounds.x() as f64, -bounds.y() as f64);
                node.draw(&context);
                surface
                    .write_to_png(
                        &mut std::fs::File::create(directory.join(format!("native-{name}.png")))
                            .unwrap(),
                    )
                    .unwrap();
            }
        }
        window.close();
        assert_eq!(
            probe.style_context().color(),
            baseline_color,
            "closing must remove search CSS"
        );
        // Reopening re-resolves style and reinstalls exactly one window's CSS.
        let reopened = build_window(&application, state.snapshot.clone());
        gtk::prelude::WidgetExt::realize(&reopened);
        let resolved = SearchStyle::load(&gtk::Entry::new());
        let actual = probe.style_context().color();
        assert!((actual.red() as f64 - resolved.foreground.red).abs() < 0.005);
        reopened.close();
        assert_eq!(probe.style_context().color(), baseline_color);
        probe_window.close();
    }

    #[test]
    #[ignore = "requires a live Hyprland session; writes a preview under target"]
    fn render_live_devices() {
        let snapshot = load_runtime(None);
        assert!(
            snapshot.binding_error.is_none(),
            "{:?}",
            snapshot.binding_error
        );
        println!(
            "{} mouse devices, {} touchpads, left placement: {}, gesture discovery: {:?}",
            snapshot.pointing.mice.len(),
            snapshot.pointing.touchpads.len(),
            snapshot.pointing.mouse_left,
            snapshot.pointing.gestures.status
        );
        let mut state = fixture_state(snapshot.layout, false, false);
        state.snapshot = snapshot;
        state.search_style = SearchStyle::load_without_gtk();
        let surface = ImageSurface::create(Format::ARgb32, 1920, 1080).unwrap();
        let context = Context::new(&surface).unwrap();
        draw_overlay(&context, 1920.0, 1080.0, &mut state);
        surface
            .write_to_png(&mut std::fs::File::create("target/keywrld-live.png").unwrap())
            .unwrap();
    }
}
