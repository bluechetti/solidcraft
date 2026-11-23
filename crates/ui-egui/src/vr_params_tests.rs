use crate::vr_params::{KEYPAD, Key, plain_number, press, step_of, stepped, typed_expression};

#[test]
fn a_plain_number_steps_by_a_unit_and_keeps_its_unit() {
    assert_eq!(stepped("12", "mm", true, false).as_deref(), Some("13 mm"));
    assert_eq!(stepped("12 mm", "mm", false, false).as_deref(), Some("11 mm"));
    assert_eq!(stepped("12mm", "mm", true, false).as_deref(), Some("13 mm"));
    assert_eq!(stepped("4", "", true, false).as_deref(), Some("5"));
}

#[test]
fn fine_steps_a_tenth_without_float_noise() {
    assert_eq!(stepped("0.2 mm", "mm", true, true).as_deref(), Some("0.3 mm"));
    assert_eq!(stepped("0.1", "mm", false, true).as_deref(), Some("0 mm"));
    assert_eq!(stepped("90 deg", "deg", true, true).as_deref(), Some("90.5 deg"));
    assert_eq!(stepped("1 in", "in", true, false).as_deref(), Some("1.0625 in"));
}

#[test]
fn a_formula_a_name_or_another_unit_is_never_stepped() {
    for e in ["width / 2", "width", "12 in", "2 * 3", "", "nan", "inf"] {
        assert_eq!(stepped(e, "mm", true, false), None, "{e}");
        assert_eq!(plain_number(e, "mm"), None, "{e}");
    }
}

#[test]
fn a_text_parameter_has_no_step() {
    assert_eq!(step_of("text", false), None);
    assert_eq!(stepped("12", "text", true, false), None);
}

#[test]
fn the_keypad_types_a_number_with_one_point_and_a_sign_in_front() {
    let mut t = String::new();
    for k in [Key::Point, Key::Digit(5), Key::Point, Key::Digit(2), Key::Minus] {
        t = press(&t, k);
    }
    assert_eq!(t, "-0.52");
    t = press(&t, Key::Minus);
    assert_eq!(t, "0.52");
    t = press(&t, Key::Back);
    assert_eq!(t, "0.5");
    assert_eq!(typed_expression(&t, "mm").as_deref(), Some("0.5 mm"));
    assert_eq!(typed_expression("-", "mm"), None);
    assert_eq!(typed_expression("", "mm"), None);
    assert_eq!(typed_expression("3", "").as_deref(), Some("3"));
}

#[test]
fn the_keypad_stops_at_its_length_and_ignores_a_bad_digit() {
    let mut t = String::new();
    for _ in 0..40 {
        t = press(&t, Key::Digit(9));
    }
    assert_eq!(t.len(), 16);
    assert_eq!(press("1", Key::Digit(12)), "1");
    assert_eq!(press("", Key::Back), "");
}

#[test]
fn the_keypad_has_every_digit_once() {
    let mut digits: Vec<u8> = KEYPAD.iter().flatten().filter_map(|k| if let Key::Digit(d) = k { Some(*d) } else { None }).collect();
    digits.sort_unstable();
    assert_eq!(digits, (0..10).collect::<Vec<u8>>());
}
