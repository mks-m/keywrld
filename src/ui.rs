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

    let provider = gtk::CssProvider::new();
    let css_color = |color: Color| {
        format!(
            "rgb({:.0}, {:.0}, {:.0})",
            color.red * 255.0,
            color.green * 255.0,
            color.blue * 255.0
        )
    };
    provider.load_from_data(&format!(
        "window.keywrld-overlay {{ background: transparent; }}
         entry.shortcut-search {{ background: {}; color: {}; border: 1px solid {};
             border-radius: 7px; padding: 5px 9px; min-height: 0; min-width: 0;
             font-size: 12px; box-shadow: none; }}
         entry.shortcut-search:focus-within {{ border-color: {}; }}",
        css_color(snapshot.palette.darker_background),
        css_color(snapshot.palette.foreground),
        css_color(snapshot.palette.muted),
        css_color(snapshot.palette.accent),
    ));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let state = Rc::new(RefCell::new(OverlayState {
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
        move |_, context, width, height| {
            draw_overlay(
                context,
                width as f64,
                height as f64,
                &mut state.borrow_mut(),
            );
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
            state.metrics.pointer = Some((x, y));
            let tooltip = state
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
                });
            drawing_area.set_tooltip_text(tooltip.as_deref());
            if state.hovered != hovered {
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
            if state.borrow().hovered.is_some() {
                state.borrow_mut().hovered = None;
                drawing_area.queue_draw();
            }
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
            let rect = search_entry_rect(layout.search);
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

    let outer_padding = 24.0;
    let header_height = 82.0;
    let detail_height = 66.0;
    let touch_rows = if state.snapshot.pointing.touchpads.is_empty() {
        Vec::new()
    } else {
        touchpad_rows(&state.snapshot.pointing, &state.snapshot.bindings)
    };
    let layout = overlay_layout(width, height, state);
    let panel = layout.keyboard;
    let unit = layout.unit;
    let board_width = state.geometry.width * unit;
    let board_height = state.geometry.height * unit;
    state.metrics.panel = panel;
    state.metrics.panels = vec![panel, layout.search];
    state.metrics.panels.extend(layout.mouse);
    state.metrics.panels.extend(layout.touchpad);
    state.metrics.hits.clear();
    state.metrics.rows.clear();

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
        content_x,
        panel.y + outer_padding,
        board_width,
        palette,
    );

    let key_gap = (unit * 0.075).clamp(3.0, 7.0);
    for key in &state.geometry.keys {
        let rect = Rect {
            x: content_x + key.rect.x * unit + key_gap / 2.0,
            y: board_y + key.rect.y * unit + key_gap / 2.0,
            width: key.rect.width * unit - key_gap,
            height: key.rect.height * unit - key_gap,
        };
        let key_actions = actions.get(&key.id);
        let hovered = state.hovered.as_deref() == Some(key.id.as_str());
        let modifier_active = modifier_key_active(&key.id, state.modifiers);
        with_icon_atlas(|icon_atlas| {
            draw_key(
                context,
                rect,
                &key.legend,
                key_actions,
                hovered,
                modifier_active,
                icon_atlas,
                palette,
            );
        });
        state.metrics.hits.push(HitRegion {
            rect,
            key: key.id.clone(),
            legend: key.legend.clone(),
        });
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

struct PanelLayout {
    keyboard: Rect,
    search: Rect,
    mouse: Option<Rect>,
    touchpad: Option<Rect>,
    unit: f64,
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

fn search_entry_rect(panel: Rect) -> Rect {
    Rect {
        x: panel.x + 12.0,
        y: panel.y + 42.0,
        width: panel.width - 24.0,
        height: 36.0,
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

fn draw_search_panel(context: &Context, panel: Rect, state: &mut OverlayState) {
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
        "SHORTCUTS",
        15.0,
        FontWeight::Bold,
        palette.foreground,
    );

    // The native entry covers this fallback in the live window. Keeping the
    // field in Cairo also makes standalone panel previews representative.
    let entry = search_entry_rect(panel);
    rounded_rectangle(context, entry, 7.0);
    set_source(context, palette.darker_background);
    let _ = context.fill_preserve();
    set_source(context, palette.accent);
    let _ = context.stroke();
    draw_text(
        context,
        entry.x + 10.0,
        entry.y + 23.0,
        &elide(
            if state.query.is_empty() {
                "Search shortcuts…"
            } else {
                &state.query
            },
            ((entry.width - 24.0) / 7.0) as usize,
        ),
        12.0,
        FontWeight::Normal,
        palette.foreground,
    );

    let rows = shortcut_rows(&state.snapshot.bindings, state.modifiers, &state.query);
    let filter = if state.modifiers.is_empty() {
        "All modifiers".into()
    } else {
        state.modifiers.label()
    };
    let noun = if rows.len() == 1 { "result" } else { "results" };
    let subtitle = format!("{} {noun} · {filter}", rows.len());
    draw_text(
        context,
        panel.x + 18.0,
        panel.y + 99.0,
        &elide(&subtitle, ((panel.width - 36.0) / 6.0) as usize),
        11.0,
        FontWeight::Normal,
        palette.dark_foreground,
    );
    let viewport = Rect {
        x: panel.x + 12.0,
        y: panel.y + 112.0,
        width: panel.width - 24.0,
        height: (panel.height - 124.0).max(0.0),
    };
    if rows.is_empty() {
        draw_text(
            context,
            viewport.x + 10.0,
            viewport.y + 24.0,
            "No matching shortcuts",
            12.0,
            FontWeight::Normal,
            palette.dark_foreground,
        );
    }
    state.metrics.search.rect = viewport;
    draw_scroll_rows(
        context,
        viewport,
        &rows,
        &mut state.metrics.search,
        &mut state.metrics.rows,
        palette,
    );
}

fn panel_layout(
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
            &elide(&row.label, ((rect.width - 20.0) / 6.0) as usize),
            10.0,
            FontWeight::Bold,
            palette.accent,
        );
        draw_text(
            context,
            rect.x + 10.0,
            rect.y + 32.0,
            &elide(&row.action, ((rect.width - 20.0) / 5.8) as usize),
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

fn draw_header(
    context: &Context,
    state: &OverlayState,
    x: f64,
    y: f64,
    width: f64,
    palette: Palette,
) {
    let title = if state.modifiers.is_empty() {
        "NO MODIFIERS".to_string()
    } else {
        state.modifiers.label()
    };
    draw_text(
        context,
        x,
        y + 24.0,
        &elide_to_width(context, &title, width, 22.0, FontWeight::Bold),
        22.0,
        FontWeight::Bold,
        palette.foreground,
    );
    draw_text(
        context,
        x,
        y + 44.0,
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
        y + 64.0,
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
    draw_text(
        context,
        rect.x + 7.0,
        rect.y + 14.0,
        legend,
        (rect.height * 0.16).clamp(8.0, 11.0),
        FontWeight::Bold,
        legend_color,
    );

    if let Some(actions) = actions {
        let icons = icons_for_actions(actions);
        if let (Some(atlas), Some(icons)) = (icon_atlas, icons) {
            draw_action_icons(
                context,
                atlas,
                Rect {
                    x: rect.x + 5.0,
                    y: rect.y + rect.height * 0.3,
                    width: rect.width - 10.0,
                    height: rect.height * 0.64,
                },
                &icons,
                palette.foreground,
            );
        } else {
            let summary = actions.join(" · ");
            let max_chars = ((rect.width - 10.0) / 6.1).floor().max(4.0) as usize;
            let lines = wrap_two_lines(&summary, max_chars);
            let font_size = (rect.height * 0.155).clamp(8.0, 11.5);
            let start_y =
                rect.y + rect.height * 0.52 - (lines.len().saturating_sub(1) as f64 * 6.0);
            for (index, line) in lines.iter().enumerate() {
                draw_text_centered(
                    context,
                    rect.x + rect.width / 2.0,
                    start_y + index as f64 * (font_size + 3.0),
                    line,
                    font_size,
                    FontWeight::Normal,
                    palette.foreground,
                );
            }
        }
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
                rect.y + 21.0,
                &elide_to_width(context, &chord, rect.width - 28.0, 12.0, FontWeight::Bold),
                12.0,
                FontWeight::Bold,
                palette.accent,
            );
            draw_text(
                context,
                rect.x + 14.0,
                rect.y + 42.0,
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
                rect.y + 33.0,
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
                rect.y + 21.0,
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
                rect.y + 42.0,
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

    #[test]
    fn panel_layout_keeps_all_devices_on_screen_and_gaps_outside_hit_regions() {
        for keyboard in [
            KeyboardLayout::Full,
            KeyboardLayout::Tkl,
            KeyboardLayout::Compact75,
            KeyboardLayout::Compact65,
            KeyboardLayout::Compact60,
        ] {
            for (width, height) in [(1920.0, 1080.0), (1280.0, 720.0), (1024.0, 600.0)] {
                for left in [false, true] {
                    for touch in [0, 20] {
                        let layout = panel_layout(
                            width,
                            height,
                            &KeyboardGeometry::new(keyboard),
                            true,
                            touch,
                            left,
                        );
                        let mut panels = vec![layout.keyboard, layout.search];
                        panels.extend(layout.mouse);
                        panels.extend(layout.touchpad);
                        for panel in &panels {
                            assert!(
                                panel.x >= 0.0
                                    && panel.y >= 0.0
                                    && panel.x + panel.width <= width + 0.01
                                    && panel.y + panel.height <= height + 0.01
                            );
                        }
                        for (i, a) in panels.iter().enumerate() {
                            for b in panels.iter().skip(i + 1) {
                                assert!(
                                    a.x + a.width <= b.x
                                        || b.x + b.width <= a.x
                                        || a.y + a.height <= b.y
                                        || b.y + b.height <= a.y
                                );
                            }
                        }
                        let mouse = layout.mouse.unwrap();
                        let keyboard = layout.keyboard;
                        let search = layout.search;
                        let neighbor = if left { mouse } else { keyboard };
                        assert!((neighbor.x - search.x - search.width - 24.0).abs() < 0.001);
                        let entry = search_entry_rect(search);
                        assert!(search.contains(entry.x, entry.y));
                        assert!(search.contains(entry.x + entry.width, entry.y + entry.height));
                        assert!(
                            (mouse.y + mouse.height / 2.0 - keyboard.y - keyboard.height / 2.0)
                                .abs()
                                < 0.001
                        );
                        let gap_x = if left {
                            mouse.x + mouse.width + 12.0
                        } else {
                            keyboard.x + keyboard.width + 12.0
                        };
                        let metrics = RenderMetrics {
                            panels,
                            ..Default::default()
                        };
                        assert!(!metrics.contains_panel(gap_x, mouse.y + mouse.height / 2.0));
                        assert!(
                            !metrics
                                .contains_panel(search.x + search.width + 12.0, search.y + 10.0)
                        );
                        assert!(metrics.contains_panel(mouse.x + 1.0, mouse.y + 1.0));
                        if let Some(pad) = layout.touchpad {
                            assert!(
                                (pad.x + pad.width / 2.0 - keyboard.x - keyboard.width / 2.0).abs()
                                    < 0.001
                            );
                            assert!(!metrics.contains_panel(pad.x + 20.0, pad.y - 12.0));
                        }
                    }
                }
            }
        }
        let layout = panel_layout(
            1280.0,
            720.0,
            &KeyboardGeometry::new(KeyboardLayout::Tkl),
            false,
            0,
            false,
        );
        assert!(layout.mouse.is_none() && layout.touchpad.is_none());
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
        ] {
            let surface = ImageSurface::create(Format::ARgb32, width, height).unwrap();
            let context = Context::new(&surface).unwrap();
            let mut state = fixture_state(layout, left, touch);
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
            assert_eq!(state.metrics.panels.len(), if touch { 4 } else { 3 });
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
        let surface = ImageSurface::create(Format::ARgb32, 1920, 1080).unwrap();
        let context = Context::new(&surface).unwrap();
        draw_overlay(&context, 1920.0, 1080.0, &mut state);
        surface
            .write_to_png(&mut std::fs::File::create("target/keywrld-live.png").unwrap())
            .unwrap();
    }
}
