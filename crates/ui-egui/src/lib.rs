//! The egui shell: theme, widgets and all screens
//!
//! Status: skeleton (planned M3.1). See the crate README and PROMPT.md for the scope.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// Icon path data generated from assets/icons by `cargo xtask icons` (regenerate, never edit).
pub mod icons_gen;

pub mod icons;
pub mod shell;
pub mod sidebar;
pub mod theme;
pub mod widgets;

pub use shell::{PostguyShell, ShellState, ShellTab};
pub use sidebar::{RequestProtocol, RowKind, Sidebar, SidebarAction, SidebarData, SidebarRow, SidebarState};
pub use theme::{Palette, Theme, ThemeMode};
pub use widgets::{
    CodeEditor, CodeLanguage, ConfirmOutcome, InlineRename, LargeTextView, MessageDirection, RenameAction, SelectGroup, SelectItem, TableVariant,
    Timeline, TimelineEntry, checkbox, compact_checkbox, context_menu, icon_button, inline_rename, key_value_table, menu_item, modal, radio,
    searchable_popup, select, split_button, switch, typed_confirm, variable_field, variable_spans,
};
