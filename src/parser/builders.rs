//! In-flight capture builders: state the parser holds while a multi-line
//! structure is open, before the finished element is emitted.

/// In-flight multi-line paired-tag capture (dialogData, compDef, ...):
/// raw lines accumulate in `buf` until `end_pattern` arrives, then the
/// assembled tag runs through the normal paired path.
#[derive(Debug, Clone)]
pub(crate) struct PairedCapture {
    pub(crate) end_pattern: &'static str,
    pub(crate) buf: String,
}

/// In-flight `<inventoryViewItem>` capture.
#[derive(Debug, Clone, Default)]
pub(crate) struct InvViewItemBuilder {
    pub(crate) token: String,
    pub(crate) exist: String,
    pub(crate) state: Option<String>,
    pub(crate) closed_attr: bool,
    pub(crate) results: Vec<(String, String)>,
    /// Some while inside a `<result>` section: (command, text so far)
    pub(crate) current: Option<(String, String)>,
}

/// In-flight `<inventoryManager>` block: children accumulate here between
/// the open and close tags (the whole response arrives on one line, but the
/// builder keeps the parser correct if a server ever splits it).
#[derive(Debug, Clone, Default)]
pub(crate) struct InvManagerBuilder {
    pub(crate) token: String,
    pub(crate) room: String,
    pub(crate) root: Option<String>,
    pub(crate) after: Option<String>,
    pub(crate) state: Option<String>,
    pub(crate) items: Vec<Vec<(String, String)>>,
    pub(crate) continuations: Vec<Vec<(String, String)>>,
}
