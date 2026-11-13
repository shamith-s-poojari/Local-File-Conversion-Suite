//! Route planner (blueprint section 3.3) and target listing for `get_targets`
//! (spec section 11.1).
//!
//! Routing rules, as implemented:
//!
//! 1. **Going back up is reconstruction.** A multi-hop path that moves up an
//!    information level (raster → fixed → structured) is capped at tier R.
//!    A single edge is trusted at its declared tier: PNG → PDF embeds pixels
//!    without inferring anything, so the matrix lists it as E.
//! 2. **Never structured → fixed or raster → structured.** Once a path
//!    leaves the structured level it may not return to it.
//! 3. **At most 3 hops**, over simple paths (no format visited twice).
//! 4. **Rank lexicographically:** worst tier, then lossy (L) hops, then hop
//!    count, then summed cost, then adapter IDs for a stable order.
//! 5. **Availability:** edges whose adapter needs a missing engine, or
//!    doesn't support this platform, are removed before planning.
//!
//! For targets shown to the user, the matrix also fixes each cell's shape
//! (see [`Planner::best_for_cell`]); the planner picks adapters, checks
//! availability and computes the tier.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::manifest::{AdapterEntry, Kind, Registry};
use crate::matrix;
use crate::model::{Format, Level, Platform, Tier};

pub const MAX_HOPS: usize = 3;

/// What this installation can run.
#[derive(Debug, Clone)]
pub struct Availability {
    pub platform: Platform,
    /// Installed engines. `None` means "assume every engine is installed".
    pub engines: Option<BTreeSet<String>>,
}

impl Availability {
    /// Every engine installed, on `platform`. Used by CI invariants.
    pub fn everything(platform: Platform) -> Self {
        Availability {
            platform,
            engines: None,
        }
    }

    fn check(&self, adapter: &AdapterEntry) -> Result<(), Unavailable> {
        let m = &adapter.manifest;
        if !m.platforms.contains(&self.platform) {
            return Err(Unavailable::PlatformUnsupported);
        }
        if let Some(installed) = &self.engines {
            if let Some(missing) = m.requires.iter().find(|e| !installed.contains(*e)) {
                return Err(Unavailable::EngineMissing {
                    engine: missing.clone(),
                });
            }
        }
        Ok(())
    }
}

/// Why a target can't be used right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "camelCase")]
pub enum Unavailable {
    PlatformUnsupported,
    #[serde(rename_all = "camelCase")]
    EngineMissing {
        engine: String,
    },
    #[serde(rename_all = "camelCase")]
    TooFewFiles {
        min: u32,
    },
    #[serde(rename_all = "camelCase")]
    TooManyFiles {
        max: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hop {
    pub adapter: String,
    pub from: Format,
    pub to: Format,
    pub tier: Tier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub hops: Vec<Hop>,
    /// The path's tier after rule 1.
    pub tier: Tier,
    pub cost: u32,
}

impl Plan {
    fn rank(&self) -> (Tier, usize, usize, u32, Vec<&str>) {
        (
            self.tier,
            self.hops.iter().filter(|h| h.tier == Tier::L).count(),
            self.hops.len(),
            self.cost,
            self.hops.iter().map(|h| h.adapter.as_str()).collect(),
        )
    }
}

#[derive(Debug, Clone)]
struct Edge {
    adapter: String,
    from: Format,
    to: Format,
    tier: Tier,
    cost: u32,
}

/// The conversion graph for one availability.
pub struct Planner {
    edges: Vec<Edge>,
}

impl Planner {
    pub fn new(registry: &Registry, availability: &Availability) -> Self {
        let mut edges = Vec::new();
        for adapter in &registry.adapters {
            let m = &adapter.manifest;
            if m.kind != Kind::Conversion || availability.check(adapter).is_err() {
                continue;
            }
            for decl in m.edges.iter().flatten() {
                for &from in &decl.from {
                    edges.push(Edge {
                        adapter: m.id.clone(),
                        from,
                        to: decl.to,
                        tier: decl.tier,
                        cost: decl.cost,
                    });
                }
            }
        }
        Planner { edges }
    }

    /// Every valid plan, best first.
    pub fn plans(&self, from: Format, to: Format) -> Vec<Plan> {
        let mut found = Vec::new();
        let mut path: Vec<&Edge> = Vec::new();
        self.walk(from, to, &mut path, &mut found);
        found.sort_by(|a, b| a.rank().cmp(&b.rank()));
        found
    }

    /// The best plan, if any.
    pub fn best(&self, from: Format, to: Format) -> Option<Plan> {
        self.plans(from, to).into_iter().next()
    }

    /// The best plan with the shape the matrix gives the cell: direct for
    /// one-hop cells, and through PDF for `²` cells. Without this, JPG → WEBP
    /// could route JPG → PNG → WEBP and beat the matrix's L with an E,
    /// changing what the product promises.
    ///
    /// A `²` cell also takes a direct edge when an adapter declares one (an
    /// office adapter that renders through PDF internally): one job instead
    /// of two, with the same intermediate.
    pub fn best_for_cell(&self, from: Format, to: Format, cell: matrix::Cell) -> Option<Plan> {
        self.plans(from, to).into_iter().find(|p| match cell.hops {
            1 => p.hops.len() == 1,
            _ => {
                p.hops.len() == 1
                    || (p.hops.len() == usize::from(cell.hops) && p.hops[0].to == Format::Pdf)
            }
        })
    }

    /// The best direct (one-hop) plan, if an adapter declares the edge.
    pub fn best_direct(&self, from: Format, to: Format) -> Option<Plan> {
        self.plans(from, to).into_iter().find(|p| p.hops.len() == 1)
    }

    fn walk<'a>(
        &'a self,
        at: Format,
        goal: Format,
        path: &mut Vec<&'a Edge>,
        found: &mut Vec<Plan>,
    ) {
        if path.len() == MAX_HOPS {
            return;
        }
        let start = path.first().map_or(at, |e| e.from);
        for edge in self.edges.iter().filter(|e| e.from == at) {
            // Simple paths only: never revisit a format.
            if edge.to == start || path.iter().any(|e| e.to == edge.to) {
                continue;
            }
            path.push(edge);
            if edge.to == goal {
                if let Some(plan) = evaluate(path) {
                    found.push(plan);
                }
            } else {
                self.walk(edge.to, goal, path, found);
            }
            path.pop();
        }
    }
}

/// Apply rules 1 and 2 to a complete path.
fn evaluate(path: &[&Edge]) -> Option<Plan> {
    let mut formats = vec![path[0].from];
    formats.extend(path.iter().map(|e| e.to));

    // Rule 2: no return to the structured level after leaving it.
    let (mut seen_structured, mut left_structured) = (false, false);
    for f in &formats {
        if f.level() == Level::Structured {
            if left_structured {
                return None;
            }
            seen_structured = true;
        } else if seen_structured {
            left_structured = true;
        }
    }

    let mut tier = path
        .iter()
        .map(|e| e.tier)
        .max()
        .expect("path is non-empty");
    // Rule 1: multi-hop paths that move up a level are reconstruction.
    let goes_up = formats.windows(2).any(|w| w[1].level() < w[0].level());
    if path.len() > 1 && goes_up {
        tier = tier.max(Tier::R);
    }

    Some(Plan {
        hops: path
            .iter()
            .map(|e| Hop {
                adapter: e.adapter.clone(),
                from: e.from,
                to: e.to,
                tier: e.tier,
            })
            .collect(),
        tier,
        cost: path.iter().map(|e| e.cost).sum(),
    })
}

// ------------------------------------------------------------------ targets

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationTarget {
    pub adapter: String,
    pub title: String,
    pub tier: Tier,
    /// Tier R targets sit behind "Show best-effort" (blueprint section 3.1).
    pub best_effort: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<Unavailable>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversionTarget {
    pub to: Format,
    pub tier: Tier,
    pub best_effort: bool,
    /// `R*` cells: exact when the PDF has a text layer.
    pub exact_with_text_layer: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<Plan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<Unavailable>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatGroup {
    pub format: Format,
    pub files: usize,
    pub conversions: Vec<ConversionTarget>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Targets {
    /// Operations that accept every selected file together.
    pub operations: Vec<OperationTarget>,
    /// Each format's own conversions (blueprint: batch target selection).
    pub groups: Vec<FormatGroup>,
    /// Conversions every group supports, such as "all to PDF", at the worst
    /// tier among the groups. Empty when only one group is selected.
    pub shared: Vec<ConversionTarget>,
}

/// Targets for a selection of files, given as their sniffed formats.
pub fn targets(registry: &Registry, availability: &Availability, files: &[Format]) -> Targets {
    let groups_formats: BTreeSet<Format> = files.iter().copied().collect();

    let mut operations = Vec::new();
    for adapter in &registry.adapters {
        let m = &adapter.manifest;
        let (Some(inputs), Some(tier)) = (&m.inputs, m.fidelity) else {
            continue;
        };
        if m.kind != Kind::Operation
            || files.is_empty()
            || !files.iter().all(|f| inputs.formats.contains(f))
        {
            continue;
        }
        let count = files.len() as u32;
        let unavailable = availability.check(adapter).err().or(if count < inputs.min {
            Some(Unavailable::TooFewFiles { min: inputs.min })
        } else if count > inputs.max {
            Some(Unavailable::TooManyFiles { max: inputs.max })
        } else {
            None
        });
        operations.push(OperationTarget {
            adapter: m.id.clone(),
            title: m.title.clone(),
            tier,
            best_effort: tier == Tier::R,
            unavailable,
        });
    }

    let installed = Planner::new(registry, availability);
    let everything = Planner::new(registry, &Availability::everything(availability.platform));
    let groups: Vec<FormatGroup> = groups_formats
        .iter()
        .map(|&format| FormatGroup {
            format,
            files: files.iter().filter(|&&f| f == format).count(),
            conversions: conversions_from(registry, availability, &installed, &everything, format),
        })
        .collect();

    let mut shared = Vec::new();
    if groups.len() > 1 {
        for to in Format::ALL {
            let per_group: Vec<&ConversionTarget> = groups
                .iter()
                .filter_map(|g| {
                    g.conversions
                        .iter()
                        .find(|c| c.to == to && c.unavailable.is_none())
                })
                .collect();
            if per_group.len() == groups.len() {
                let tier = per_group.iter().map(|c| c.tier).max().expect("non-empty");
                shared.push(ConversionTarget {
                    to,
                    tier,
                    best_effort: tier == Tier::R,
                    exact_with_text_layer: false,
                    plan: None,
                    unavailable: None,
                });
            }
        }
    }

    Targets {
        operations,
        groups,
        shared,
    }
}

fn conversions_from(
    registry: &Registry,
    availability: &Availability,
    installed: &Planner,
    everything: &Planner,
    from: Format,
) -> Vec<ConversionTarget> {
    let mut out = Vec::new();
    for to in Format::ALL {
        let cell = if from.in_blueprint() && to.in_blueprint() {
            match matrix::offered(from, to) {
                Some(cell) => cell,
                None => continue,
            }
        } else {
            // ADR 0003: outside the blueprint matrix, a declared direct edge
            // is the offer, at its declared tier.
            match everything.best_direct(from, to) {
                Some(plan) => matrix::Cell {
                    tier: plan.tier,
                    hops: 1,
                    exact_with_text_layer: false,
                },
                None => continue,
            }
        };
        let target =
            |plan: Option<Plan>, unavailable: Option<Unavailable>, tier: Tier| ConversionTarget {
                to,
                tier,
                best_effort: tier == Tier::R,
                exact_with_text_layer: cell.exact_with_text_layer,
                plan,
                unavailable,
            };
        if let Some(plan) = installed.best_for_cell(from, to, cell) {
            let tier = plan.tier;
            out.push(target(Some(plan), None, tier));
        } else if let Some(plan) = everything.best_for_cell(from, to, cell) {
            // Offered by an adapter whose engine isn't installed here.
            let reason = plan
                .hops
                .iter()
                .find_map(|h| {
                    registry
                        .get(&h.adapter)
                        .and_then(|a| availability.check(a).err())
                })
                .unwrap_or(Unavailable::PlatformUnsupported);
            out.push(target(None, Some(reason), plan.tier));
        }
        // Otherwise no adapter implements this cell yet, so it isn't listed.
    }
    out
}
