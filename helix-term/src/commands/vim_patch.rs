use std::sync::atomic::{AtomicBool, Ordering};

use crate::commands::*;
use helix_core::graphemes::prev_grapheme_boundary;
use helix_core::line_ending::rope_is_line_ending;
use helix_core::{Range, RopeSlice};
use helix_view::document::Mode;

#[derive(Default)]
pub struct AtomicState {
    visual_lines: AtomicBool,
    highlight: AtomicBool,
}

pub static VIM_STATE: AtomicState = AtomicState::new();

impl AtomicState {
    pub const fn new() -> Self {
        Self {
            visual_lines: AtomicBool::new(false),
            highlight: AtomicBool::new(false),
        }
    }

    pub fn visual_line(&self) {
        self.visual_lines.store(true, Ordering::Relaxed);
    }

    pub fn exit_visual_line(&self) {
        self.visual_lines.store(false, Ordering::Relaxed);
    }

    pub fn is_visual_line(&self) -> bool {
        self.visual_lines.load(Ordering::Relaxed)
    }

    pub fn allow_highlight(&self) {
        self.highlight.store(true, Ordering::Relaxed);
    }

    pub fn stop_highlight(&self) {
        self.highlight.store(false, Ordering::Relaxed);
    }

    pub fn is_highlight(&self) -> bool {
        self.highlight.load(Ordering::Relaxed)
    }
}

pub struct VimOps;

impl VimOps {
    pub fn hook_after_each_command(cx: &mut Context) {
        if cx.editor.mode != Mode::Select {
            if !VIM_STATE.is_highlight() {
                collapse_selection(cx);
            } else {
                VIM_STATE.stop_highlight();
            }
        } else {
            // check if visual lines
            if VIM_STATE.is_visual_line() {
                extend_to_line_bounds(cx);
            }
        }
    }
}

macro_rules! wrap_hooks {
    // with both before and after
    ($wrapper:ident, $func:path, before = $before:expr, after = $after:expr) => {
        pub fn $wrapper(cx: &mut Context) {
            $before(cx);
            $func(cx);
            $after(cx);
        }
    };

    // with only before
    ($wrapper:ident, $func:path, before = $before:expr) => {
        pub fn $wrapper(cx: &mut Context) {
            $before(cx);
            $func(cx);
        }
    };

    // with only after
    ($wrapper:ident, $func:path, after = $after:expr) => {
        pub fn $wrapper(cx: &mut Context) {
            $func(cx);
            $after(cx);
        }
    };
}

macro_rules! wrap_many_with_hooks {
    (
        [ $( ( $wrapper:ident, $func:path ) ),+ $(,)? ],
        before = $before:expr,
        after = $after:expr
    ) => {
        $(
            wrap_hooks!($wrapper, $func, before = $before, after = $after);
        )+
    };

    (
        [ $( ( $wrapper:ident, $func:path ) ),+ $(,)? ],
        before = $before:expr
    ) => {
        $(
            wrap_hooks!($wrapper, $func, before = $before);
        )+
    };

    (
        [ $( ( $wrapper:ident, $func:path ) ),+ $(,)? ],
        after = $after:expr
    ) => {
        $(
            wrap_hooks!($wrapper, $func, after = $after);
        )+
    };
}

#[macro_export]
macro_rules! static_commands_with_default {
    ($macro_to_call:ident! ( $($name:ident, $doc:literal,)* )) => {
        $macro_to_call! {
        vim_visual_lines, "Visual lines (vim)",
        vim_normal_mode, "Normal mode (vim)",
        vim_exit_select_mode, "Exit select mode (vim)",
        vim_move_next_word_start, "Move to start of next word (vim)",
        vim_move_next_long_word_start, "Move next long word (vim)",
        vim_extend_next_word_start, "Extend to start of next word (vim)",
        vim_extend_next_long_word_start, "Extend to start of next long word (vim)",
        vim_extend_visual_line_up, "Move up (vim)",
        vim_extend_visual_line_down, "Move down (vim)",
        vim_goto_line, "Go to line (vim)",
        vim_move_paragraph_forward, "Move by paragraph forward (vim)",
        vim_move_paragraph_backward, "Move by paragraph forward (vim)",
        vim_cursor_forward_search, "Forward search for word near cursor (vim)",
        vim_cursor_backward_search, "Backward search for word near cursor (vim)",
            $($name, $doc,)*
        }
    };
}

pub use vim_commands::*;

mod vim_commands {
    use vim_patch::exit_select_mode;

    use super::*;

    pub fn vim_visual_lines(cx: &mut Context) {
        select_mode(cx);
        VIM_STATE.visual_line();
        extend_to_line_bounds(cx);
    }

    wrap_many_with_hooks!(
        [
            (vim_move_next_word_start, move_next_word_start),
            (vim_move_next_long_word_start, move_next_long_word_start),
        ],
        after = move_char_right
    );

    wrap_many_with_hooks!(
        [
            (vim_extend_next_word_start, extend_next_word_start),
            (vim_extend_next_long_word_start, extend_next_long_word_start),
        ],
        after = extend_char_right
    );

    pub fn vim_goto_line(cx: &mut Context) {
        if cx.count.is_none() {
            goto_last_line(cx);
        } else {
            goto_line(cx);
        }
    }

    pub fn vim_extend_visual_line_down(cx: &mut Context) {
        if VIM_STATE.is_visual_line() {
            extend_line_down(cx);
        } else {
            extend_visual_line_down(cx);
        }
    }

    pub fn vim_extend_visual_line_up(cx: &mut Context) {
        if VIM_STATE.is_visual_line() {
            extend_line_up(cx);
        } else {
            extend_visual_line_up(cx);
        }
    }

    pub fn vim_normal_mode(cx: &mut Context) {
        normal_mode(cx);
        VIM_STATE.exit_visual_line();
    }

    pub fn vim_exit_select_mode(cx: &mut Context) {
        exit_select_mode(cx);
        VIM_STATE.exit_visual_line();
    }

    pub fn vim_move_paragraph_forward(cx: &mut Context) {
        goto_para_impl(cx, vim_utils::movement_paragraph_forward);
        if cx.editor.mode != Mode::Select {
            normal_mode(cx);
        }
    }

    pub fn vim_move_paragraph_backward(cx: &mut Context) {
        goto_para_impl(cx, vim_utils::movement_paragraph_backward);
        if cx.editor.mode != Mode::Select {
            normal_mode(cx);
        }
    }

    pub fn vim_cursor_forward_search(cx: &mut Context) {
        VIM_STATE.allow_highlight();
        vim_utils::cursor_search_impl(cx, Direction::Forward);
    }

    pub fn vim_cursor_backward_search(cx: &mut Context) {
        VIM_STATE.allow_highlight();
        vim_utils::cursor_search_impl(cx, Direction::Backward);
    }
}

mod vim_utils {
    use super::*;

    pub fn movement_paragraph_backward(
        slice: RopeSlice,
        range: Range,
        count: usize,
        behavior: Movement,
    ) -> Range {
        //Mostly copy/past from Movements::move_prev_paragraph
        let mut line = range.cursor_line(slice);
        let first_char = slice.line_to_char(line) == range.cursor(slice);
        let prev_line_empty = rope_is_line_ending(slice.line(line.saturating_sub(1)));
        let curr_line_empty = rope_is_line_ending(slice.line(line));
        let prev_empty_to_line = prev_line_empty && !curr_line_empty;

        // skip character before paragraph boundary
        if prev_empty_to_line && !first_char {
            line += 1;
        }
        let mut lines = slice.lines_at(line);
        lines.reverse();
        let mut lines = lines.map(rope_is_line_ending).peekable();
        let mut last_line = line;
        for _ in 0..count {
            while lines.next_if(|&e| e).is_some() {
                line -= 1;
            }
            while lines.next_if(|&e| !e).is_some() {
                line -= 1;
            }
            if lines.next_if(|&e| e).is_some() {
                line -= 1;
            }
            if line == last_line {
                break;
            }
            last_line = line;
        }

        let head = slice.line_to_char(line);
        let anchor = if behavior == Movement::Move {
            // exclude first character after paragraph boundary
            if prev_empty_to_line && first_char {
                range.cursor(slice)
            } else {
                range.head
            }
        } else {
            range.put_cursor(slice, head, true).anchor
        };
        Range::new(anchor, head)
    }

    pub fn movement_paragraph_forward(
        slice: RopeSlice,
        range: Range,
        count: usize,
        behavior: Movement,
    ) -> Range {
        //Mostly copy/paste from Movements::move_next_paragraph
        let mut line = range.cursor_line(slice);
        let last_char =
            prev_grapheme_boundary(slice, slice.line_to_char(line + 1)) == range.cursor(slice);
        let curr_line_empty = rope_is_line_ending(slice.line(line));
        let next_line_empty =
            rope_is_line_ending(slice.line(slice.len_lines().saturating_sub(1).min(line + 1)));
        let curr_empty_to_line = curr_line_empty && !next_line_empty;

        // skip character after paragraph boundary
        if curr_empty_to_line && last_char {
            line += 1;
        }
        let mut lines = slice.lines_at(line).map(rope_is_line_ending).peekable();
        let mut last_line = line;
        for _ in 0..count {
            while lines.next_if(|&e| e).is_some() {
                line += 1;
            }
            while lines.next_if(|&e| !e).is_some() {
                line += 1;
            }
            if lines.next_if(|&e| e).is_some() {
                line += 1;
            }
            if line == last_line {
                break;
            }
            last_line = line;
        }
        let head = slice.line_to_char(line);
        let anchor = if behavior == Movement::Move {
            if curr_empty_to_line && last_char {
                range.head
            } else {
                range.cursor(slice)
            }
        } else {
            range.put_cursor(slice, head, true).anchor
        };
        Range::new(anchor, head)
    }

    pub fn cursor_search_impl(cx: &mut Context, direction: Direction) {
        fn find_keyword_char(slice: RopeSlice) -> Option<usize> {
            slice
                .chars()
                .position(|ch| ch.is_alphanumeric() || ch == '_')
        }
        fn goto_next_keyword_char_in_line(view: &mut View, doc: &mut Document) {
            let text = doc.text().slice(..);

            let selection = doc.selection(view.id).clone().transform(|range| {
                let line = range.cursor_line(text);
                let line_start = text.line_to_char(line);

                let pos_end =
                    graphemes::prev_grapheme_boundary(text, line_end_char_index(&text, line))
                        .max(line_start);

                let anchor = range.cursor(text);
                let search_limit = (pos_end + 1).min(text.len_chars());
                if let Some(pos) = find_keyword_char(text.slice(anchor..search_limit)) {
                    range.put_cursor(text, anchor + pos, false)
                } else {
                    range.put_cursor(text, anchor, false)
                }
            });
            doc.set_selection(view.id, selection);
        }

        exit_select_mode(cx);
        keep_primary_selection(cx);

        let count = cx.count();
        let (view, doc) = current!(cx.editor);
        goto_next_keyword_char_in_line(view, doc);

        let text = doc.text().slice(..);
        let selection = doc.selection(view.id);

        if selection.primary().fragment(text).trim().is_empty() {
            cx.editor.set_error("No string under cursor");
            return;
        }

        // Use Helix 'word' as a Vim 'keyword' equivalent
        let objtype = textobject::TextObject::Inside;
        let selection = selection
            .clone()
            .transform(|range| textobject::textobject_word(text, range, objtype, count, false));
        doc.set_selection(view.id, selection);
        search_selection_detect_word_boundaries(cx);

        let config = cx.editor.config();
        if config.search.smart_case {
            // Make the search case insensitive by prepending (?i) to the regex
            let register = cx.register.unwrap_or('/');
            let regex = match cx.editor.registers.first(register, cx.editor) {
                Some(regex) => format!("(?i){}", regex),
                None => return,
            };

            let msg = format!("register '{}' set to '{}'", register, &regex);
            match cx.editor.registers.push(register, regex) {
                Ok(_) => {
                    cx.editor.registers.last_search_register = register;
                    cx.editor.set_status(msg)
                }
                Err(err) => {
                    cx.editor
                        .set_error(format!("Failed to update register: {}", err));
                    return;
                }
            }
        }
        search_next_or_prev_impl(cx, Movement::Move, direction);
    }
}
