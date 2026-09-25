use super::*;
use crate::bottom_pane::textarea::TextArea;
use pretty_assertions::assert_eq;

#[test]
fn shell_completion_popup_snapshot() {
    let popup = ShellCompletionPopup {
        text: "!echo".to_string(),
        cursor: 5,
        lines: vec![ShellMenuLine {
            text: "echo  echotc  echoti".to_string(),
            selected_cells: Some(6..12),
        }],
    };
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 3)).expect("terminal");
    terminal
        .draw(|frame| popup.render_menu(frame.area(), frame.buffer_mut()))
        .expect("draw popup");
    insta::assert_snapshot!("shell_completion_popup", terminal.backend());
    let selected_style = crate::style::selection_style();
    let selected_cells = (1_u16..39)
        .map(|x| {
            if terminal
                .backend()
                .buffer()
                .cell((x, 1))
                .is_some_and(|cell| cell.style() == selected_style)
            {
                '#'
            } else {
                '.'
            }
        })
        .collect::<String>();
    insta::assert_snapshot!("shell_completion_selected_cells", selected_cells);
}

#[test]
fn dismissed_tokens_after_adjacent_elements_are_occurrence_scoped() {
    let first_bound = "$bound1";
    let second_bound = "$bound2";
    let token = "$other";
    let text = format!("{first_bound}{token} {second_bound}{token}");
    let first_start = first_bound.len();
    let second_start = text.rfind(token).expect("second token");
    let second_bound_start = text.find(second_bound).expect("second bound mention");
    let mut textarea = TextArea::new();
    textarea.insert_str(&text);
    textarea.add_element_range(0..first_start);
    textarea.add_element_range(second_bound_start..second_bound_start + second_bound.len());
    let dismissed = DismissedToken::new(
        &textarea,
        first_start..first_start + token.len(),
        "other".to_string(),
    );

    assert!(!dismissed.matches(
        &textarea,
        &(second_start..second_start + token.len()),
        "other",
    ));
}

#[test]
fn nested_at_query_does_not_count_as_complete_dismissed_token() {
    let text = "@ma@latest @ma";
    let second_start = text.rfind("@ma").expect("second token");
    let mut textarea = TextArea::new();
    textarea.insert_str(text);

    assert_eq!(
        complete_token_occurrences_before(&textarea, "@ma", second_start),
        0
    );
}

#[test]
fn non_sigil_elements_delimit_dismissed_token_occurrences() {
    let token = "$other";
    let first_element = "[one]";
    let text = format!("{token}{first_element} {token}");
    let second_start = text.rfind(token).expect("second token");
    let mut textarea = TextArea::new();
    textarea.insert_str(&text);
    textarea.add_element_range(token.len()..token.len() + first_element.len());
    let dismissed = DismissedToken::new(&textarea, 0..token.len(), "other".to_string());

    assert!(!dismissed.matches(
        &textarea,
        &(second_start..second_start + token.len()),
        "other",
    ));
}
