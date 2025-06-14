use std::sync::atomic::{AtomicBool, Ordering};

use crate::commands::*;
use helix_view::document::Mode;

#[derive(Default)]
pub struct AtomicState {
    visual_lines: AtomicBool,
}

pub static VIM_STATE: AtomicState = AtomicState::new();

impl AtomicState {
    pub const fn new() -> Self {
        Self {
            visual_lines: AtomicBool::new(false),
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
}

pub struct VimOps;

impl VimOps {
    pub fn hook_after_each_command(cx: &mut Context) {
        if cx.editor.mode != Mode::Select {
            collapse_selection(cx);
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
}
