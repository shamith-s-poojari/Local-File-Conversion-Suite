//! What the core tells the output re-check (ADR 0006).
//!
//! A tool whose manifest declares `recheck` can't succeed until a separate
//! check job, run by the scheduler over copies of its outputs, has passed
//! them. What that job is told is built here, from the user's own params and
//! the core's own facts, never from anything the tool's worker said.

use converter_router::manifest::CHECK_ADAPTER;
use converter_router::{AdapterEntry, RecheckKind};
use converter_scheduler::Recheck;
use serde_json::{json, Map, Value};

use crate::{App, CommandError, CommandResult, Reason};

impl App {
    /// The re-check for a job part that runs `entry`, if it declares one.
    ///
    /// Refused at creation, not at the end of a long job, when the check
    /// can't run here (its adapter or an engine is missing).
    pub(crate) fn recheck_for(
        &self,
        entry: &AdapterEntry,
        file_ids: &[String],
        params: &Value,
    ) -> CommandResult<Option<Recheck>> {
        let Some(kind) = entry.manifest.recheck else {
            return Ok(None);
        };
        let checker = self
            .registry
            .get(CHECK_ADAPTER)
            .ok_or_else(|| CommandError::with("E_INSTALL_CORRUPT", Reason::UnknownAdapter))?;
        if self.availability_of(checker).is_err() {
            return Err(CommandError::with(
                "E_INSTALL_CORRUPT",
                Reason::EngineMissing,
            ));
        }
        let (params, failure_code) = match kind {
            RecheckKind::Redaction => {
                // The page count of the file as it was before the tool ran.
                let pages = match file_ids {
                    [one] => self.inspect_file(one)?.pages,
                    _ => 0,
                };
                (
                    redaction_params(params, pages)
                        .ok_or_else(|| CommandError::with("E_INTERNAL", Reason::WrongFileCount))?,
                    "E_REDACTION_VERIFICATION_FAILED",
                )
            }
            RecheckKind::ActiveContent => {
                (json!({ "verifier": "activeContent" }), "E_OUTPUT_INVALID")
            }
        };
        let limits = &checker.manifest.limits;
        Ok(Some(Recheck {
            adapter_id: checker.manifest.id.clone(),
            timeout_base_s: limits.timeout_base_s,
            timeout_s_per_mb: limits.timeout_s_per_mb,
            no_progress_s: limits.no_progress_s,
            params,
            failure_code: failure_code.to_owned(),
        }))
    }
}

/// The user's boxes, terms and fill, and the original page count, as
/// `output-check` takes them. `None` if the page count is unknown (zero).
///
/// The tool's worker also checks the text it captured under each box. The
/// core never has that text, so that one check (V9) isn't asked for here.
fn redaction_params(params: &Value, pages: u64) -> Option<Value> {
    if pages == 0 {
        return None;
    }
    let mut out = Map::new();
    out.insert("verifier".into(), json!("redaction"));
    out.insert("expectedPages".into(), json!(pages));
    let fill = match params["fill"].as_str() {
        Some("white") => [255, 255, 255],
        _ => [0, 0, 0],
    };
    out.insert("fill".into(), json!(fill));
    out.insert(
        "terms".into(),
        params
            .get("terms")
            .filter(|t| t.is_array())
            .cloned()
            .unwrap_or_else(|| json!([])),
    );
    // Boxes can be drawn from any corner; the check wants them ordered.
    let regions: Vec<Value> = params["boxes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| {
            let n = |k: &str| b[k].as_f64();
            let (x0, y0, x1, y1) = (n("x0")?, n("y0")?, n("x1")?, n("y1")?);
            Some(json!({
                "page": b["page"].as_u64()?,
                "x0": x0.min(x1), "y0": y0.min(y1), "x1": x0.max(x1), "y1": y0.max(y1),
            }))
        })
        .collect();
    out.insert("regions".into(), Value::Array(regions));
    Some(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_params_come_from_the_users_own_params() {
        let params = json!({
            "boxes": [{"page": 2, "x0": 0.5, "y0": 0.4, "x1": 0.1, "y1": 0.2}],
            "terms": ["secret"],
            "fill": "white",
            "rasterizeFallback": true,
        });
        let got = redaction_params(&params, 3).unwrap();
        assert_eq!(
            got,
            json!({
                "verifier": "redaction",
                "expectedPages": 3,
                "fill": [255, 255, 255],
                "terms": ["secret"],
                "regions": [{"page": 2, "x0": 0.1, "y0": 0.2, "x1": 0.5, "y1": 0.4}],
            })
        );
        // Never the captured text, which the core doesn't have.
        assert!(got.get("captured").is_none());
    }

    #[test]
    fn defaults_are_black_and_no_terms() {
        let got = redaction_params(&json!({"boxes": []}), 1).unwrap();
        assert_eq!(got["fill"], json!([0, 0, 0]));
        assert_eq!(got["terms"], json!([]));
        assert_eq!(got["regions"], json!([]));
    }

    #[test]
    fn an_unknown_page_count_is_refused_rather_than_skipped() {
        assert!(redaction_params(&json!({}), 0).is_none());
    }
}
