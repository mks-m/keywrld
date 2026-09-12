use gtk4::cairo::{Context, ImageSurface};

use crate::core::Color;

const TILE_SIZE: i32 = 64;
const ATLAS_COLUMNS: i32 = 8;
const ATLAS_ROWS: i32 = 16;
const ATLAS_BYTES: &[u8] = include_bytes!("../assets/icons/action-icons.png");

thread_local! {
    static ICON_ATLAS: Option<IconAtlas> = IconAtlas::load();
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum ActionIcon {
    // This order must stay in sync with assets/icons/sources.txt.
    Ellipsis,
    Terminal,
    Globe,
    Folder,
    Music,
    Editor,
    Key,
    Calendar,
    Mail,
    MailPlus,
    Video,
    Image,
    Map,
    Bot,
    Message,
    Close,
    Copy,
    Paste,
    Cut,
    Clipboard,
    Menu,
    Apps,
    Emoji,
    Camera,
    Cpu,
    Keyboard,
    Calculator,
    PanelTop,
    Palette,
    Transparency,
    BellMinus,
    BellOff,
    BellRing,
    History,
    LockKeyhole,
    Nightlight,
    Laptop,
    Monitor,
    Pipette,
    ScanText,
    Share,
    AlarmPlus,
    Alarm,
    AlarmOff,
    Clock,
    Battery,
    Weather,
    Volume,
    VolumeOff,
    Bluetooth,
    Wifi,
    Power,
    Activity,
    ZoomIn,
    ZoomOut,
    Reset,
    Fullscreen,
    Split,
    Tiles,
    PictureInPicture,
    Save,
    Restore,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Horizontal,
    Vertical,
    Previous,
    Next,
    Front,
    ResizeHorizontal,
    ResizeVertical,
    Scratchpad,
    Group,
    GroupCycle,
    Workspace,
    Move,
    Send,
    Silent,
    Download,
    Link,
    Delete,
    CloseAll,
    ScreenShare,
    Microphone,
    MicrophoneOff,
    Brightness,
    Pointer,
    Play,
    Pause,
    NextTrack,
    PreviousTrack,
    Eject,
    Audio,
    Theme,
    Square,
    Gaps,
    Transcode,
    Bar,
    Lock,
    Window,
    Layers,
    Minimize,
    Maximize,
    Toggle,
    Record,
    FileText,
    BrandOmarchy,
    BrandSignal,
    BrandObsidian,
    BrandYoutube,
    BrandWhatsapp,
    BrandGoogleMessages,
    BrandGooglePhotos,
    BrandGoogleMaps,
    BrandX,
    BrandDocker,
    BrandTmux,
    BrandSpotify,
    BrandOnePassword,
    BrandGoogleCalendar,
    BrandGmail,
}

pub(crate) struct IconAtlas {
    surface: ImageSurface,
}

impl IconAtlas {
    pub(crate) fn load() -> Option<Self> {
        let mut bytes = ATLAS_BYTES;
        let surface = ImageSurface::create_from_png(&mut bytes).ok()?;
        if surface.width() != ATLAS_COLUMNS * TILE_SIZE
            || surface.height() != ATLAS_ROWS * TILE_SIZE
        {
            return None;
        }
        Some(Self { surface })
    }

    pub(crate) fn draw(
        &self,
        context: &Context,
        icon: ActionIcon,
        x: f64,
        y: f64,
        size: f64,
        color: Color,
    ) {
        if size <= 0.0 || context.save().is_err() {
            return;
        }

        let index = icon as i32;
        let tile_x = (index % ATLAS_COLUMNS) * TILE_SIZE;
        let tile_y = (index / ATLAS_COLUMNS) * TILE_SIZE;

        context.rectangle(x, y, size, size);
        context.clip();
        context.translate(x, y);
        let scale = size / f64::from(TILE_SIZE);
        context.scale(scale, scale);
        context.set_source_rgba(color.red, color.green, color.blue, color.alpha);
        let _ = context.mask_surface(&self.surface, -f64::from(tile_x), -f64::from(tile_y));
        let _ = context.restore();
    }
}

pub(crate) fn with_icon_atlas<T>(callback: impl FnOnce(Option<&IconAtlas>) -> T) -> T {
    ICON_ATLAS.with(|atlas| callback(atlas.as_ref()))
}

pub(crate) fn icons_for_actions(actions: &[String]) -> Option<Vec<ActionIcon>> {
    if actions.is_empty() {
        return None;
    }

    let mut icons = Vec::new();
    for action in actions {
        let icon = icon_for_action(action)?;
        if !icons.contains(&icon) {
            icons.push(icon);
        }
    }

    if icons.len() > 2 {
        icons.truncate(1);
        icons.push(ActionIcon::Ellipsis);
    }
    Some(icons)
}

fn icon_for_action(action: &str) -> Option<ActionIcon> {
    let normalized = action.trim().to_ascii_lowercase();
    let action = normalized.as_str();

    // Exact brands come first so a brand name cannot be swallowed by a broad
    // semantic rule. Brands absent from the pinned release use generic icons.
    match action {
        "omarchy menu" => return Some(ActionIcon::BrandOmarchy),
        "signal" => return Some(ActionIcon::BrandSignal),
        "obsidian" => return Some(ActionIcon::BrandObsidian),
        "youtube" => return Some(ActionIcon::BrandYoutube),
        "whatsapp" => return Some(ActionIcon::BrandWhatsapp),
        "google messages" => return Some(ActionIcon::BrandGoogleMessages),
        "google photos" => return Some(ActionIcon::BrandGooglePhotos),
        "google maps" => return Some(ActionIcon::BrandGoogleMaps),
        "x" | "x post" => return Some(ActionIcon::BrandX),
        "docker" => return Some(ActionIcon::BrandDocker),
        "tmux" | "tmux keybindings" => return Some(ActionIcon::BrandTmux),
        "music" => return Some(ActionIcon::BrandSpotify),
        "passwords" | "1password" => return Some(ActionIcon::BrandOnePassword),
        "google calendar" => return Some(ActionIcon::BrandGoogleCalendar),
        "gmail" => return Some(ActionIcon::BrandGmail),
        _ => {}
    }

    match action {
        "terminal" | "herdr" => Some(ActionIcon::Terminal),
        "music tui" => Some(ActionIcon::Music),
        "ssh" => Some(ActionIcon::Key),
        "chatgpt" | "grok" | "agent" => Some(ActionIcon::Bot),
        "calendar" => Some(ActionIcon::Calendar),
        "email" => Some(ActionIcon::Mail),
        "new email" => Some(ActionIcon::MailPlus),
        "editor" | "typora" | "omawrite" => Some(ActionIcon::Editor),
        "calculator" => Some(ActionIcon::Calculator),
        "emojis" => Some(ActionIcon::Emoji),
        "audio" => Some(ActionIcon::Audio),
        "bluetooth" => Some(ActionIcon::Bluetooth),
        "display" => Some(ActionIcon::Monitor),
        "network" => Some(ActionIcon::Wifi),
        "power" => Some(ActionIcon::Power),
        "activity" => Some(ActionIcon::Activity),
        "share" => Some(ActionIcon::Share),
        "transcode" => Some(ActionIcon::Transcode),
        "screenshot" => Some(ActionIcon::Camera),
        "screenrecording" => Some(ActionIcon::Record),
        "color picker" => Some(ActionIcon::Pipette),
        "toggle touchpad" | "enable touchpad" | "disable touchpad" => Some(ActionIcon::Pointer),
        "play" => Some(ActionIcon::Play),
        "pause" => Some(ActionIcon::Pause),
        "next track" => Some(ActionIcon::NextTrack),
        "previous track" => Some(ActionIcon::PreviousTrack),
        "eject media" => Some(ActionIcon::Eject),
        "switch audio output" | "switch media source" => Some(ActionIcon::Audio),
        "universal copy" => Some(ActionIcon::Copy),
        "universal paste" => Some(ActionIcon::Paste),
        "universal cut" => Some(ActionIcon::Cut),
        "clipboard manager" => Some(ActionIcon::Clipboard),
        "toggle top bar" => Some(ActionIcon::Bar),
        "background switcher" => Some(ActionIcon::Image),
        "theme menu" => Some(ActionIcon::Palette),
        "theme" => Some(ActionIcon::Theme),
        "toggle window transparency" => Some(ActionIcon::Transparency),
        "toggle window gaps" => Some(ActionIcon::Gaps),
        "toggle single-window square aspect" => Some(ActionIcon::Square),
        "dismiss last notification" => Some(ActionIcon::BellMinus),
        "dismiss all notifications" => Some(ActionIcon::Delete),
        "toggle silencing notifications" => Some(ActionIcon::BellOff),
        "invoke last notification" => Some(ActionIcon::BellRing),
        "open notification history" => Some(ActionIcon::History),
        "toggle locking on idle" => Some(ActionIcon::LockKeyhole),
        "toggle nightlight" => Some(ActionIcon::Nightlight),
        "toggle laptop display" => Some(ActionIcon::Laptop),
        "toggle laptop display mirroring" => Some(ActionIcon::ScreenShare),
        "extract text (ocr) from screenshot" => Some(ActionIcon::ScanText),
        "set reminder" => Some(ActionIcon::AlarmPlus),
        "show reminders" => Some(ActionIcon::Alarm),
        "clear reminders" => Some(ActionIcon::AlarmOff),
        "show time" => Some(ActionIcon::Clock),
        "show battery remaining" => Some(ActionIcon::Battery),
        "toggle weather" => Some(ActionIcon::Weather),
        "zoom in" => Some(ActionIcon::ZoomIn),
        "reset zoom" => Some(ActionIcon::Reset),
        "lock system" => Some(ActionIcon::Lock),
        "close window" => Some(ActionIcon::Close),
        "close all windows" => Some(ActionIcon::CloseAll),
        "toggle window split" => Some(ActionIcon::Split),
        "pseudo window" => Some(ActionIcon::Layers),
        "toggle window floating/tiling" => Some(ActionIcon::Window),
        "full screen" | "tiled full screen" => Some(ActionIcon::Fullscreen),
        "full width" => Some(ActionIcon::Horizontal),
        "pop window out (float & pin)" => Some(ActionIcon::PictureInPicture),
        "save window width" => Some(ActionIcon::Save),
        "restore window width" => Some(ActionIcon::Restore),
        "toggle workspace layout" => Some(ActionIcon::Tiles),
        "toggle scratchpad" => Some(ActionIcon::Scratchpad),
        "move window to scratchpad" => Some(ActionIcon::Send),
        "next workspace" | "scroll active workspace forward" => Some(ActionIcon::Next),
        "previous workspace" | "scroll active workspace backward" => Some(ActionIcon::Previous),
        "former workspace" => Some(ActionIcon::Restore),
        "focus on next window" | "focus on next monitor" => Some(ActionIcon::Next),
        "focus on previous window" | "focus on previous monitor" => Some(ActionIcon::Previous),
        "reveal active window on top" => Some(ActionIcon::Front),
        "move window" => Some(ActionIcon::Move),
        "resize window" => Some(ActionIcon::ResizeHorizontal),
        "toggle window grouping" | "move active window out of group" => Some(ActionIcon::Group),
        "next window in group" => Some(ActionIcon::Next),
        "previous window in group" => Some(ActionIcon::Previous),
        "monitor scaling up" => Some(ActionIcon::ZoomIn),
        "monitor scaling down" => Some(ActionIcon::ZoomOut),
        "capture highlighted window" | "capture entire screen" => Some(ActionIcon::Camera),
        "select next window to capture" => Some(ActionIcon::Next),
        "select previous window to capture" => Some(ActionIcon::Previous),
        "select window to capture" => Some(ActionIcon::Camera),
        "download video from web app" => Some(ActionIcon::Download),
        "copy url from web app" => Some(ActionIcon::Link),
        _ => icon_for_action_family(action),
    }
}

fn icon_for_action_family(action: &str) -> Option<ActionIcon> {
    if action.starts_with("browser") {
        return Some(ActionIcon::Globe);
    }
    if action.starts_with("file manager") {
        return Some(ActionIcon::Folder);
    }
    if action.contains("keybindings") {
        return Some(ActionIcon::Keyboard);
    }
    if action.starts_with("apps menu") {
        return Some(ActionIcon::Apps);
    }
    if action.starts_with("capture menu") {
        return Some(ActionIcon::Camera);
    }
    if action.starts_with("hardware menu") {
        return Some(ActionIcon::Cpu);
    }
    if action.starts_with("system menu") || action.starts_with("power menu") {
        return Some(ActionIcon::Power);
    }
    if action.ends_with(" menu") {
        return Some(ActionIcon::Menu);
    }
    if action.starts_with("bar panel ") {
        return Some(ActionIcon::PanelTop);
    }
    if action.contains("video") {
        return Some(ActionIcon::Video);
    }
    if action.contains("photo") {
        return Some(ActionIcon::Image);
    }
    if action.contains("map") {
        return Some(ActionIcon::Map);
    }
    if action.contains("message") {
        return Some(ActionIcon::Message);
    }
    if action.contains("vertical") {
        return Some(ActionIcon::Vertical);
    }
    if action.starts_with("volume ") {
        return Some(if action.contains("mute") {
            ActionIcon::VolumeOff
        } else {
            ActionIcon::Volume
        });
    }
    if action == "mute" {
        return Some(ActionIcon::VolumeOff);
    }
    if action.contains("microphone") || action.contains("dictation") {
        return Some(if action.contains("mute") || action.contains("stop") {
            ActionIcon::MicrophoneOff
        } else {
            ActionIcon::Microphone
        });
    }
    if action.contains("brightness") || action.contains("backlight") {
        return Some(if action.contains("keyboard") {
            ActionIcon::Keyboard
        } else {
            ActionIcon::Brightness
        });
    }
    if action.starts_with("make webcam overlay smaller") {
        return Some(ActionIcon::Minimize);
    }
    if action.starts_with("make webcam overlay larger") {
        return Some(ActionIcon::Maximize);
    }
    if action.starts_with("switch to workspace ") {
        return Some(ActionIcon::Workspace);
    }
    if action.starts_with("move window silently to workspace ") {
        return Some(ActionIcon::Silent);
    }
    if action.starts_with("move window to workspace ") {
        return Some(ActionIcon::Send);
    }
    if action.starts_with("move workspace to ")
        || action.starts_with("swap window")
        || action.starts_with("focus on ")
        || action.starts_with("move window to group on ")
        || action.starts_with("move grouped window focus ")
    {
        return directional_icon(action);
    }
    if action.starts_with("expand window") || action.starts_with("shrink window") {
        return Some(if action.contains(" left") || action.contains(" right") {
            ActionIcon::ResizeHorizontal
        } else {
            ActionIcon::ResizeVertical
        });
    }
    if action.starts_with("switch to group window ") {
        return Some(ActionIcon::GroupCycle);
    }
    if action.contains("window in group") || action.contains("window grouping") {
        return Some(ActionIcon::Group);
    }
    if action.starts_with("download ") {
        return Some(ActionIcon::Download);
    }
    if action.starts_with("copy url") {
        return Some(ActionIcon::Link);
    }
    if action.contains("screenshot") {
        return Some(ActionIcon::Camera);
    }
    if action.contains("record") {
        return Some(ActionIcon::Record);
    }
    if action.contains("text") || action.contains("document") {
        return Some(ActionIcon::FileText);
    }
    if action.starts_with("toggle ") {
        return Some(ActionIcon::Toggle);
    }
    None
}

fn directional_icon(action: &str) -> Option<ActionIcon> {
    let words: Vec<_> = action.split_whitespace().collect();
    if words.iter().any(|word| *word == "left") {
        Some(ActionIcon::ArrowLeft)
    } else if words.iter().any(|word| *word == "right") {
        Some(ActionIcon::ArrowRight)
    } else if words
        .iter()
        .any(|word| matches!(*word, "up" | "above" | "top"))
    {
        Some(ActionIcon::ArrowUp)
    } else if words
        .iter()
        .any(|word| matches!(*word, "down" | "below" | "bottom"))
    {
        Some(ActionIcon::ArrowDown)
    } else {
        None
    }
}
