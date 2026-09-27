//! Explorer effects: what the runtime does for the pure update.

use super::history::HistoryEntry;
use crate::domain::types::http::HttpRequest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExplorerEffect {
    /// Load the API description (`spec_loader`).
    LoadSpec,
    /// Read when the stored token of the API's OAuth source ends. Reads
    /// only: nothing is refreshed.
    ReadTokenExpiry,
    /// Send the request through `ApiRuntime::call`, which adds the credential.
    SendRequest(HttpRequest),
    /// Open `$EDITOR` on the text, as a file of the given type (`json`,
    /// `txt`), and return what was saved.
    EditBody {
        text: String,
        suffix: &'static str,
    },
    CopyToClipboard {
        text: String,
    },
    /// Copy the jq path of a tree node.
    CopyPath {
        path: String,
    },
    OpenBrowser {
        url: String,
    },
    /// Run a jq filter on the response body.
    ApplyJq {
        filter: String,
        body: Vec<u8>,
    },
    PreviewJq {
        filter: String,
        body: std::sync::Arc<serde_json::Value>,
    },
    /// Read the API's request history. Reads only: nothing is sent.
    LoadHistory,
    /// Append one line to the API's request history.
    AppendHistory(HistoryEntry),
    Exit,
}
