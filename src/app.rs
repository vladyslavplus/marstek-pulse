use slint::ComponentHandle;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrayAvailability {
    Unknown,
    Available,
    Unavailable,
}

fn should_show_main_window(start_hidden: bool, availability: TrayAvailability) -> bool {
    !start_hidden || availability != TrayAvailability::Available
}

fn should_hide_on_close(availability: TrayAvailability) -> bool {
    availability == TrayAvailability::Available
}

fn request_exit() {
    let _ = slint::quit_event_loop();
}
slint::include_modules!();

pub fn run(start_hidden: bool) -> Result<(), slint::PlatformError> {
    let window = Monitor::new()?;
    let mut tray = Tray::new().ok();
    if let Some(icon) = tray.as_ref()
        && icon.show().is_err()
    {
        tray = None;
    }
    // Slint 1.18.1 exposes no result for native tray registration.
    let availability = if tray.is_some() {
        TrayAvailability::Unknown
    } else {
        TrayAvailability::Unavailable
    };
    let availability = Rc::new(Cell::new(availability));
    if let Some(icon) = tray.as_ref() {
        let weak = window.as_weak();
        let availability_for_show = availability.clone();
        icon.on_show_window(move || {
            availability_for_show.set(TrayAvailability::Available);
            if let Some(window) = weak.upgrade() {
                let _ = window.show();
            }
        });
        let weak = window.as_weak();
        let availability_for_hide = availability.clone();
        icon.on_hide_window(move || {
            availability_for_hide.set(TrayAvailability::Available);
            if let Some(window) = weak.upgrade() {
                let _ = window.hide();
            }
        });
        icon.on_exit_app(request_exit);
    }
    window.on_exit_app(request_exit);
    window.on_ukrainian(|| {
        let _ = slint::select_bundled_translation("uk");
    });
    window.on_english(|| {
        let _ = slint::select_bundled_translation("en");
    });
    let availability_for_close = availability.clone();
    window.window().on_close_requested(move || {
        if should_hide_on_close(availability_for_close.get()) {
            slint::CloseRequestResponse::HideWindow
        } else {
            slint::CloseRequestResponse::KeepWindowShown
        }
    });
    if should_show_main_window(start_hidden, availability.get()) {
        window.show()?;
    }
    slint::run_event_loop()?;
    if let Some(icon) = tray {
        icon.hide()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{TrayAvailability, should_hide_on_close, should_show_main_window};

    #[test]
    fn unknown_or_unavailable_tray_keeps_window_reachable() {
        for availability in [TrayAvailability::Unknown, TrayAvailability::Unavailable] {
            assert!(should_show_main_window(true, availability));
            assert!(!should_hide_on_close(availability));
        }
    }

    #[test]
    fn verified_tray_allows_hidden_start_and_close_to_tray() {
        assert!(!should_show_main_window(true, TrayAvailability::Available));
        assert!(should_show_main_window(false, TrayAvailability::Available));
        assert!(should_hide_on_close(TrayAvailability::Available));
    }
}
