//! Scribble: write a value into a number field with the Apple Pencil.
//!
//! iPadOS turns handwriting into text only for its own text inputs, and SolidCraft draws its
//! fields itself. So winit's view gets a `UIIndirectScribbleInteraction`: its delegate tells
//! iPadOS where the number fields are ([`solidcraft_ui_egui::numeric_fields`], updated each frame
//! by [`set_fields`]), and when writing starts in one it gives that field the keyboard focus in the
//! app and lays an invisible `UITextField` over it for iPadOS to write into. What is written
//! replaces the field's value ([`take`] hands it to the app as select-all and the text).
//!
//! UIKit has no safe Rust API, so (like `picker` and `keyboard`) this module allows `unsafe`.
#![allow(unsafe_code)]
use std::cell::RefCell;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSArray, NSNotification, NSNotificationCenter, NSNumber, NSObject, NSObjectProtocol, NSString};
use objc2_ui_kit::{
    UIColor, UIIndirectScribbleInteraction, UIIndirectScribbleInteractionDelegate, UIKeyboardType, UIResponder, UITextField,
    UITextFieldTextDidChangeNotification, UITextInputTraits, UIView,
};

/// A number field's id (`egui::Id::value`) and its rect in points.
type Field = (u64, egui::Rect);

#[derive(Default)]
struct State {
    /// The number fields the last frame showed.
    fields: Vec<Field>,
    /// The field that has the keyboard focus in the app.
    focused: Option<u64>,
    /// A field writing started in: the app gives it the focus next frame.
    focus: Option<u64>,
    /// Text written since the last frame, replacing the field's value.
    written: Option<String>,
    /// The invisible text field iPadOS writes into, and the field it covers.
    proxy: Option<Retained<UITextField>>,
    writing_in: Option<u64>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
    static INSTALLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static KEEP: RefCell<Vec<Retained<NSObject>>> = const { RefCell::new(Vec::new()) };
    static OBSERVER: RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>> = const { RefCell::new(None) };
}

fn cg_rect(r: egui::Rect) -> CGRect {
    CGRect {
        origin: CGPoint { x: f64::from(r.min.x), y: f64::from(r.min.y) },
        size: CGSize { width: f64::from(r.width()), height: f64::from(r.height()) },
    }
}

fn element_id(element: &AnyObject) -> Option<u64> {
    let number: &NSNumber = element.downcast_ref()?;
    Some(number.unsignedLongLongValue())
}

fn field(id: u64) -> Option<egui::Rect> {
    STATE.with(|s| s.borrow().fields.iter().find(|(f, _)| *f == id).map(|(_, r)| *r))
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SolidCraftScribbleDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UIIndirectScribbleInteractionDelegate for Delegate {
        #[unsafe(method(indirectScribbleInteraction:requestElementsInRect:completion:))]
        fn request_elements(
            &self,
            _interaction: &UIIndirectScribbleInteraction,
            rect: CGRect,
            completion: &block2::DynBlock<dyn Fn(NonNull<NSArray<AnyObject>>)>,
        ) {
            let area = egui::Rect::from_min_size(
                egui::pos2(rect.origin.x as f32, rect.origin.y as f32),
                egui::vec2(rect.size.width as f32, rect.size.height as f32),
            );
            let ids: Vec<Retained<AnyObject>> = STATE.with(|s| {
                s.borrow()
                    .fields
                    .iter()
                    .filter(|(_, r)| r.intersects(area))
                    .map(|(id, _)| Retained::into_super(Retained::into_super(Retained::into_super(NSNumber::new_u64(*id)))))
                    .collect()
            });
            let array = NSArray::from_retained_slice(&ids);
            completion.call((NonNull::from(&*array),));
        }

        #[unsafe(method(indirectScribbleInteraction:isElementFocused:))]
        fn is_focused(&self, _interaction: &UIIndirectScribbleInteraction, element: &AnyObject) -> bool {
            let id = element_id(element);
            STATE.with(|s| {
                let s = s.borrow();
                id.is_some() && s.writing_in == id && s.proxy.as_ref().is_some_and(|p| p.isFirstResponder())
            })
        }

        #[unsafe(method(indirectScribbleInteraction:frameForElement:))]
        fn frame(&self, _interaction: &UIIndirectScribbleInteraction, element: &AnyObject) -> CGRect {
            element_id(element).and_then(field).map_or(CGRect::default(), cg_rect)
        }

        #[unsafe(method(indirectScribbleInteraction:focusElementIfNeeded:referencePoint:completion:))]
        fn focus(
            &self,
            _interaction: &UIIndirectScribbleInteraction,
            element: &AnyObject,
            _point: CGPoint,
            completion: &block2::DynBlock<dyn Fn(*mut UIResponder)>,
        ) {
            let responder = element_id(element).and_then(|id| Some((id, field(id)?))).and_then(|(id, rect)| {
                let proxy = STATE.with(|s| s.borrow().proxy.clone())?;
                proxy.setFrame(cg_rect(rect));
                proxy.setText(Some(&NSString::from_str("")));
                STATE.with(|s| {
                    let mut s = s.borrow_mut();
                    s.writing_in = Some(id);
                    s.focus = Some(id);
                });
                proxy.becomeFirstResponder();
                Some(Retained::into_super(Retained::into_super(Retained::into_super(proxy))))
            });
            let ptr = responder.as_ref().map_or(std::ptr::null_mut(), |r: &Retained<UIResponder>| Retained::as_ptr(r).cast_mut());
            completion.call((ptr,));
        }

        #[unsafe(method(indirectScribbleInteraction:didFinishWritingInElement:))]
        fn finished(&self, _interaction: &UIIndirectScribbleInteraction, _element: &AnyObject) {
            // The value is in the app's field: the proxy steps aside so the app's own view takes
            // the keyboard again (OK, Enter or a tap elsewhere ends the edit as usual).
            STATE.with(|s| {
                let mut s = s.borrow_mut();
                s.writing_in = None;
                if let Some(proxy) = &s.proxy {
                    proxy.resignFirstResponder();
                }
            });
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: `init` on an allocated NSObject subclass.
        unsafe { msg_send![super(this), init] }
    }
}

/// Add the interaction to the app's view. Safe to call every frame; does the work once.
pub fn install(view: &UIView) {
    if INSTALLED.with(std::cell::Cell::get) {
        return;
    }
    let Some(mtm) = MainThreadMarker::new() else { return };
    INSTALLED.with(|i| i.set(true));
    let delegate = Delegate::new(mtm);
    let interaction =
        UIIndirectScribbleInteraction::initWithDelegate(UIIndirectScribbleInteraction::alloc(mtm), &ProtocolObject::from_retained(delegate.clone()));
    view.addInteraction(ProtocolObject::from_ref(&*interaction));
    // The proxy: invisible, but in the view so iPadOS can write into it and show its preview.
    let proxy = UITextField::new(mtm);
    proxy.setTextColor(Some(&UIColor::clearColor()));
    // SAFETY: a plain colour for the caret.
    unsafe { proxy.setTintColor(Some(&UIColor::clearColor())) };
    proxy.setKeyboardType(UIKeyboardType::NumbersAndPunctuation);
    // No Scribble shortcut bar (undo, redo, keyboard) over the field, and no system keyboard:
    // the app's own number pad and buttons stand for them.
    let assistant = proxy.inputAssistantItem();
    assistant.setLeadingBarButtonGroups(&NSArray::new());
    assistant.setTrailingBarButtonGroups(&NSArray::new());
    let empty = UIView::new(mtm);
    // SAFETY: an empty view as the input view: the field takes text without a keyboard.
    unsafe { proxy.setInputView(Some(&empty)) };
    view.addSubview(&proxy);
    let center = NSNotificationCenter::defaultCenter();
    let block = block2::RcBlock::new(move |note: NonNull<NSNotification>| {
        // SAFETY: the notification outlives the block call.
        let note = unsafe { note.as_ref() };
        let Some(field) = note.object() else { return };
        let Some(field) = field.downcast_ref::<UITextField>() else { return };
        let text = field.text().map(|t| t.to_string()).unwrap_or_default();
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            if s.proxy.as_deref().is_some_and(|p| std::ptr::eq(p, field)) {
                s.written = Some(text);
            }
        });
    });
    // SAFETY: the block lives as long as the observer, which is the app's life.
    let observer =
        unsafe { center.addObserverForName_object_queue_usingBlock(Some(UITextFieldTextDidChangeNotification), Some(&proxy), None, &block) };
    STATE.with(|s| s.borrow_mut().proxy = Some(proxy));
    KEEP.with(|k| {
        let mut k = k.borrow_mut();
        k.push(Retained::into_super(interaction));
        k.push(Retained::into_super(delegate));
    });
    OBSERVER.with(|o| *o.borrow_mut() = Some(observer));
}

/// The number fields this frame showed and the one with the focus, after the app's UI has run.
pub fn set_fields(ctx: &egui::Context) {
    let fields: Vec<Field> = solidcraft_ui_egui::numeric_fields(ctx).into_iter().map(|(id, r)| (id.value(), r)).collect();
    let focused = ctx.memory(|m| m.focused()).map(|id| id.value());
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.fields = fields;
        s.focused = focused;
    });
}

/// Before the app reads the frame's input: give the field writing started in the focus, and
/// type what was written into it (select all, then the text, so it replaces the value).
pub fn take(ctx: &egui::Context, raw: &mut egui::RawInput) {
    let (focus, written) = STATE.with(|s| {
        let mut s = s.borrow_mut();
        (s.focus.take(), s.written.take())
    });
    if let Some(id) = focus
        && let Some(field) = solidcraft_ui_egui::numeric_fields(ctx).into_iter().find(|(f, _)| f.value() == id)
    {
        ctx.memory_mut(|m| m.request_focus(field.0));
    }
    if let Some(text) = written {
        let select_all = egui::Modifiers::COMMAND;
        for pressed in [true, false] {
            raw.events.push(egui::Event::Key { key: egui::Key::A, physical_key: Some(egui::Key::A), pressed, repeat: false, modifiers: select_all });
        }
        if text.is_empty() {
            for pressed in [true, false] {
                raw.events.push(egui::Event::Key {
                    key: egui::Key::Backspace,
                    physical_key: Some(egui::Key::Backspace),
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                });
            }
        } else {
            raw.events.push(egui::Event::Text(text));
        }
        ctx.request_repaint();
    }
}
