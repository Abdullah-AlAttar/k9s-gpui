//! Domain-free GPUI components: theme, tokens, data table.

mod data_table;
pub mod theme;

pub use data_table::{DEFAULT_FONT_PX, DataTable, FONT, Open, bindings as data_table_bindings};
