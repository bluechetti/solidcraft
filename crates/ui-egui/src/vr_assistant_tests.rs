use crate::vr_assistant::heard;

#[test]
fn what_was_heard_is_one_line_without_silence_markers() {
    assert_eq!(heard("  make the posts\n 10 mm taller \n").as_deref(), Some("make the posts 10 mm taller"));
    assert_eq!(heard("[BLANK_AUDIO]\n"), None);
    assert_eq!(heard(" [Music] fillet it "), Some("fillet it".into()));
    assert_eq!(heard(""), None);
}
