//! Hand-drawn icons for the window chrome, lighter than their Lucide twins.

use gpui_kit::component::Icon;

/// macOS-style sidebar: a wide frame with a narrow left pane.
pub fn sidebar() -> Icon {
    Icon::default().data(include_bytes!("../assets/icons/sidebar.svg"))
}

pub fn search() -> Icon {
    Icon::default().data(include_bytes!("../assets/icons/search.svg"))
}

pub fn qr_code() -> Icon {
    Icon::default().data(include_bytes!("../assets/icons/qr-code.svg"))
}
