use crate::vr_assistant::{Review, heard, review};

#[test]
fn what_was_heard_is_one_line_without_silence_markers() {
    assert_eq!(heard("  make the posts\n 10 mm taller \n").as_deref(), Some("make the posts 10 mm taller"));
    assert_eq!(heard("[BLANK_AUDIO]\n"), None);
    assert_eq!(heard(" [Music] fillet it "), Some("fillet it".into()));
    assert_eq!(heard(""), None);
}

#[test]
fn keep_and_undo_wait_for_the_agent_to_have_worked_and_changed_the_design() {
    // Just sent: queued, not yet working, nothing changed: wait (not "done without a change").
    assert_eq!(review(3, false, false, 3), Review::Wait);
    assert_eq!(review(3, true, true, 4), Review::Wait, "still working");
    assert_eq!(review(3, true, false, 4), Review::Offer);
    assert_eq!(review(3, true, false, 3), Review::Forget, "done, nothing changed");
}
