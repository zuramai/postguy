//! The egui shell: theme, widgets and all screens
//!
//! Status: skeleton (planned M3.1). See the crate README and PROMPT.md for the scope.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

/// Icon path data generated from assets/icons by `cargo xtask icons` (regenerate, never edit).
pub mod icons_gen;

pub mod auth;
pub mod builder;
pub mod collection;
pub mod commands;
pub mod console;
pub mod cookies;
pub mod environment;
pub mod icons;
pub mod response;
pub mod settings;
pub mod shell;
pub mod sidebar;
pub mod snippet;
pub mod tabs;
pub mod theme;
pub mod trash;
pub mod widgets;

pub use builder::{BodyDrafts, BuilderAction, BuilderTab, RequestBuilder, SendProgress};
pub use collection::{CollectionAction, CollectionDetail, DetailState, DetailTab};
pub use commands::{Command, CommandAction, CommandRegistry, PaletteItem, PaletteState, command_palette};
pub use console::{ConsoleAction, ConsoleFilter, ConsoleRow, ConsoleState, console_view};
pub use cookies::{CookieAction, CookieData, CookieRow, CookieState, cookie_manager};
pub use environment::{EnvironmentAction, EnvironmentEditor, VariableGroup, variable_quick_look};
pub use response::{BodyMode, ResponseAction, ResponseData, ResponseTab, ResponseViewerState, response_view};
pub use settings::{SettingsAction, SettingsEditor, SettingsSection, SettingsState};
pub use shell::{PostguyShell, ShellState};
pub use sidebar::{RequestProtocol, RowKind, Sidebar, SidebarAction, SidebarData, SidebarRow, SidebarState};
pub use snippet::{SnippetLanguage, SnippetPanelState, snippet_panel};
pub use tabs::{CloseChoice, Tab, TabAction, TabDoc, Tabs, tab_strip};
pub use theme::{Palette, Theme, ThemeMode};
pub use trash::{TrashAction, TrashEntryView, trash_view};
pub use widgets::{
    CodeEditor, CodeLanguage, ConfirmOutcome, InlineRename, LargeTextView, MessageDirection, RenameAction, SelectGroup, SelectItem, TableVariant,
    Timeline, TimelineEntry, checkbox, compact_checkbox, context_menu, icon_button, inline_rename, key_value_table, menu_item, modal, radio,
    searchable_popup, select, split_button, switch, typed_confirm, variable_field, variable_spans,
};
