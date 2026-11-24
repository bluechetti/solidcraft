//! Stored preferences on the XR page (`PAGE_XR`).
//!
//! Each entry is one field of `Prefs`, in `preference_settings!`'s form:
//! `field: Type = default, PAGE[, accept(range)][ => control(...)];`. Add a new preference
//! to the file of the page whose Defaults resets it, in alphabetical order by field name, so
//! two new preferences rarely land on the same line.

macro_rules! xr_preferences {
    ($next:ident $(, $rest:ident)* ; $($fields:tt)*) => {
        $next! { $($rest),* ; $($fields)*
            /// VR's buttons and ways of working (Preferences › XR › VR, `vr_settings`): applied at once,
            /// also with the headset on. The defaults are what VR did before they were settings.
            vr: crate::vr_settings::VrSettings = Default::default(), PAGE_XR;
            /// Start in VR from the next start (#2172, docs/vr.md). Off by default: the app starts
            /// as it always has, with no VR software touched.
            vr_headset: bool = false, PAGE_XR => check_id("prefs.vr_headset");
        }
    };
}
