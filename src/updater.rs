use std::{
    cell::{Cell, OnceCell},
    ffi::CString,
    os::unix::ffi::OsStrExt,
    ptr,
    time::Duration,
};

use futures::{StreamExt, channel::mpsc};
use gpui_kit::{App, Global};
use objc2::{
    DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyClass, AnyObject, Bool, NSObject},
};
use objc2_foundation::NSString;

const CHECK_INTERVAL: Duration = Duration::from_secs(4 * 60 * 60);
const BUSY_RETRY_DELAY: Duration = Duration::from_secs(30);

// NS_ENUM values from Sparkle's SPUUpdateCheck and SPUUserUpdateChoice.
const SCHEDULED_CHECK: isize = 1;
const INFORMATION_CHECK: isize = 2;
const SKIP_UPDATE: isize = 0;

#[derive(PartialEq, Eq)]
pub struct AvailableUpdate(pub Option<String>);

impl Global for AvailableUpdate {}

enum Event {
    Availability(AvailableUpdate),
    CheckRequested,
}

struct DelegateState {
    events: mpsc::UnboundedSender<Event>,
    probing: Cell<bool>,
    user_requested_check: Cell<bool>,
}

define_class!(
    // NSObject has no subclassing requirements, and this delegate lives on the main thread.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateState]
    struct UpdateDelegate;

    impl UpdateDelegate {
        #[unsafe(method(updater:mayPerformUpdateCheck:error:))]
        fn may_check(&self, _updater: &AnyObject, kind: isize, _error: *mut *mut AnyObject) -> Bool {
            // Older releases may have enabled Sparkle's own scheduled alerts in preferences.
            Bool::new(kind != SCHEDULED_CHECK)
        }

        #[unsafe(method(updater:didFindValidUpdate:))]
        fn found_update(&self, _updater: &AnyObject, item: &AnyObject) {
            // A valid informational-only item can require a manual download on a website.
            let information_only: Bool = unsafe { msg_send![item, isInformationOnlyUpdate] };
            let version = if information_only.as_bool() {
                None
            } else {
                let version: Retained<NSString> = unsafe { msg_send![item, displayVersionString] };
                Some(version.to_string())
            };
            self.send_availability(version);
        }

        #[unsafe(method(updaterDidNotFindUpdate:))]
        fn no_update(&self, _updater: &AnyObject) {
            self.send_availability(None);
        }

        #[unsafe(method(updater:userDidMakeChoice:forUpdate:state:))]
        fn made_choice(&self, _updater: &AnyObject, choice: isize, _item: &AnyObject, _state: &AnyObject) {
            if choice == SKIP_UPDATE {
                self.send_availability(None);
            }
        }

        #[unsafe(method(updater:didFinishUpdateCycleForUpdateCheck:error:))]
        fn finished_check(&self, _updater: &AnyObject, kind: isize, _error: *const AnyObject) {
            if kind == INFORMATION_CHECK {
                self.ivars().probing.set(false);
                if self.ivars().user_requested_check.replace(false) {
                    let _ = self.ivars().events.unbounded_send(Event::CheckRequested);
                }
            }
        }
    }
);

impl UpdateDelegate {
    fn new(main_thread: MainThreadMarker, events: mpsc::UnboundedSender<Event>) -> Retained<Self> {
        let delegate = Self::alloc(main_thread).set_ivars(DelegateState {
            events,
            probing: Cell::new(false),
            user_requested_check: Cell::new(false),
        });
        unsafe { msg_send![super(delegate), init] }
    }

    fn send_availability(&self, version: Option<String>) {
        let _ = self
            .ivars()
            .events
            .unbounded_send(Event::Availability(AvailableUpdate(version)));
    }
}

struct Updater {
    controller: Retained<AnyObject>,
    delegate: Retained<UpdateDelegate>,
}

thread_local! {
    static UPDATER: OnceCell<Updater> = const { OnceCell::new() };
}

pub fn start(cx: &mut App) -> bool {
    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    let (sender, mut events) = mpsc::unbounded();
    let Some(updater) = load_controller(main_thread, sender) else {
        return false;
    };
    if UPDATER.with(|slot| slot.set(updater)).is_err() {
        return true;
    }

    cx.set_global(AvailableUpdate(None));
    cx.spawn(async move |cx| {
        while let Some(event) = events.next().await {
            cx.update(|cx| match event {
                Event::Availability(update) => {
                    if cx.global::<AvailableUpdate>() != &update {
                        cx.set_global(update);
                    }
                }
                Event::CheckRequested => check(),
            });
        }
    })
    .detach();
    cx.spawn(async |cx| {
        loop {
            let delay = if probe() {
                CHECK_INTERVAL
            } else {
                BUSY_RETRY_DELAY
            };
            cx.background_executor().timer(delay).await;
        }
    })
    .detach();
    true
}

pub fn check() {
    UPDATER.with(|slot| {
        if let Some(updater) = slot.get() {
            if updater.delegate.ivars().probing.get() {
                updater.delegate.ivars().user_requested_check.set(true);
            } else {
                // The controller is created on the main thread and takes a nullable sender.
                unsafe {
                    msg_send![&*updater.controller, checkForUpdates: ptr::null::<AnyObject>()]
                }
            }
        }
    });
}

fn probe() -> bool {
    UPDATER.with(|slot| {
        let Some(updater) = slot.get() else {
            return false;
        };
        let sparkle: Retained<AnyObject> = unsafe { msg_send![&*updater.controller, updater] };
        let busy: Bool = unsafe { msg_send![&*sparkle, sessionInProgress] };
        if busy.as_bool() {
            return false;
        }
        updater.delegate.ivars().probing.set(true);
        // The informational check uses Sparkle's signed feed and skips versions the user declined.
        unsafe { msg_send![&*sparkle, checkForUpdateInformation] }
        true
    })
}

fn load_controller(
    main_thread: MainThreadMarker,
    events: mpsc::UnboundedSender<Event>,
) -> Option<Updater> {
    let executable = std::env::current_exe().ok()?;
    let framework = executable
        .parent()?
        .parent()?
        .join("Frameworks/Sparkle.framework/Sparkle");
    if !framework.is_file() {
        return None;
    }
    let path = CString::new(framework.as_os_str().as_bytes()).ok()?;
    // Load the embedded, codesigned framework by absolute path. It stays loaded
    // while its Objective-C classes are registered with the runtime.
    if unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) }.is_null() {
        let error = unsafe { libc::dlerror() };
        if !error.is_null() {
            eprintln!(
                "CodeAny updater: {}",
                unsafe { std::ffi::CStr::from_ptr(error) }.to_string_lossy()
            );
        }
        return None;
    }
    let class = AnyClass::get(c"SPUStandardUpdaterController")?;
    let delegate = UpdateDelegate::new(main_thread, events);
    // The controller weakly references its delegate, so keep both alive for the application.
    let controller: Option<Retained<AnyObject>> = unsafe {
        msg_send![msg_send![class, alloc],
            initWithStartingUpdater: Bool::YES,
            updaterDelegate: &*delegate,
            userDriverDelegate: ptr::null::<AnyObject>()
        ]
    };
    Some(Updater {
        controller: controller?,
        delegate,
    })
}
