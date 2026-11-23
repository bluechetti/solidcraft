//! SolidCraft: the UIScene life cycle.
//!
//! Apps built with the iOS 27 SDK must declare a scene (`UIApplicationSceneManifest` in their
//! `Info.plist`); without one the app never becomes active, so winit never resumes and no window
//! is made. The manifest names [`WinitSceneDelegate`], which keeps the window scene iOS connects;
//! windows are then made in it (`WinitUIWindow::new`). An app without the manifest gets no scene
//! and keeps the old, screen-based windows.
use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{declare_class, mutability, ClassType, DeclaredClass};

thread_local! {
    static SCENE: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
}

declare_class!(
    pub(crate) struct WinitSceneDelegate;

    unsafe impl ClassType for WinitSceneDelegate {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "WinitSceneDelegate";
    }

    impl DeclaredClass for WinitSceneDelegate {}

    unsafe impl WinitSceneDelegate {
        #[method(scene:willConnectToSession:options:)]
        fn will_connect(&self, scene: &AnyObject, _session: &AnyObject, _options: &AnyObject) {
            // SAFETY: `scene` is a live UIWindowScene for the call; keeping a strong reference
            // keeps it for the app's life, which is the scene's.
            let scene = unsafe { Retained::retain(scene as *const AnyObject as *mut AnyObject) };
            SCENE.with(|s| *s.borrow_mut() = scene);
        }
    }
);

/// Register the delegate class, so UIKit finds it by the name in the manifest. Call before
/// `UIApplicationMain`.
pub(crate) fn register() {
    let class = WinitSceneDelegate::class();
    // UIKit requires the class to declare `UIWindowSceneDelegate` (and so `UISceneDelegate`).
    if let Some(protocol) = objc2::runtime::AnyProtocol::get("UIWindowSceneDelegate") {
        // SAFETY: adds a protocol the class implements the needed methods of; both are valid
        // runtime objects for the app's life.
        unsafe {
            objc2::ffi::class_addProtocol(
                class as *const objc2::runtime::AnyClass as *mut objc2::ffi::objc_class,
                protocol as *const objc2::runtime::AnyProtocol as *const objc2::ffi::objc_protocol,
            );
        }
    }
}

/// The window scene iOS connected, if the app declares one.
pub(crate) fn connected() -> Option<Retained<AnyObject>> {
    SCENE.with(|s| s.borrow().clone())
}
