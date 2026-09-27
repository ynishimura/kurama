//! `Gate`: one line of a report -- a step's name, whether it passed and
//! what it found -- and how the reports print it.

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Gate {
    pub(crate) name: String,
    pub(crate) ok: bool,
    pub(crate) detail: String,
}

impl Gate {
    /// A gate that did not run, and why: it passes, and the report says
    /// NOT_RUN instead of pass.
    pub(crate) fn not_run(name: &str, reason: &str) -> Gate {
        Gate {
            name: name.to_string(),
            ok: true,
            detail: format!("NOT_RUN: {reason}"),
        }
    }

    /// The gates as the table every report opens with.
    pub(crate) fn markdown_table(gates: &[Gate]) -> String {
        let mut out = String::from("| Gate | Result | Detail |\n| --- | --- | --- |\n");
        for gate in gates {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                gate.name,
                gate.result(),
                gate.detail
            ));
        }
        out
    }

    pub(crate) fn is_not_run(&self) -> bool {
        self.detail.starts_with("NOT_RUN:")
    }

    pub(crate) fn result(&self) -> &'static str {
        if self.is_not_run() {
            "NOT_RUN"
        } else if self.ok {
            "pass"
        } else {
            "FAIL"
        }
    }
}
