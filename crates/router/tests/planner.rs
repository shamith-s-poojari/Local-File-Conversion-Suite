//! Route planner: the blueprint's CI invariants (section 3.3) over the
//! fixture manifests, plus availability and target listing.

use std::collections::BTreeSet;
use std::path::Path;

use converter_router::matrix;
use converter_router::planner::Unavailable;
use converter_router::{
    build_registry, targets, Availability, Catalog, Format, Level, Planner, Platform, Registry,
    Tier,
};

use Format::*;

fn registry() -> Registry {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let catalog = Catalog::load(&root.join("../..")).unwrap();
    build_registry(&root.join("tests/fixtures/adapters"), &catalog, false).unwrap()
}

fn all(platform: Platform) -> Availability {
    Availability::everything(platform)
}

fn without(engines: &[&str]) -> Availability {
    let r = registry();
    let mut installed: BTreeSet<String> = r
        .adapters
        .iter()
        .flat_map(|a| a.manifest.requires.iter().cloned())
        .collect();
    for e in engines {
        installed.remove(*e);
    }
    Availability {
        platform: Platform::Linux,
        engines: Some(installed),
    }
}

// ------------------------------------------------- blueprint CI invariants

#[test]
fn every_offered_cell_yields_a_plan_at_its_tier() {
    let r = registry();
    for platform in [Platform::Windows, Platform::Macos, Platform::Linux] {
        let planner = Planner::new(&r, &all(platform));
        for (from, to, cell) in matrix::all_offered() {
            let plan = planner
                .best_for_cell(from, to, cell)
                .unwrap_or_else(|| panic!("{from:?} -> {to:?}: no plan on {platform:?}"));
            assert_eq!(plan.tier, cell.tier, "{from:?} -> {to:?}: plan {plan:?}");
            assert_eq!(
                plan.hops.len(),
                usize::from(cell.hops),
                "{from:?} -> {to:?}"
            );
        }
    }
}

#[test]
fn no_plan_breaks_rules_1_to_3() {
    let planner = Planner::new(&registry(), &all(Platform::Linux));
    for from in Format::ALL {
        for to in Format::ALL {
            if from == to {
                continue;
            }
            for plan in planner.plans(from, to) {
                let path: Vec<Format> = std::iter::once(from)
                    .chain(plan.hops.iter().map(|h| h.to))
                    .collect();
                // Rule 3: at most 3 hops, simple path.
                assert!(plan.hops.len() <= 3, "{path:?}");
                let unique: BTreeSet<_> = path.iter().collect();
                assert_eq!(unique.len(), path.len(), "revisits a format: {path:?}");
                // Hops connect.
                for w in plan.hops.windows(2) {
                    assert_eq!(w[0].to, w[1].from);
                }
                // Rule 2: never back to structured after leaving it.
                let first_non_structured = path
                    .iter()
                    .position(|f| f.level() != Level::Structured)
                    .unwrap_or(path.len());
                let started_structured = path[0].level() == Level::Structured;
                if started_structured {
                    assert!(
                        path[first_non_structured..]
                            .iter()
                            .all(|f| f.level() != Level::Structured),
                        "rule 2: {path:?}"
                    );
                }
                // Rule 1: multi-hop paths going up are capped at R.
                let goes_up = path.windows(2).any(|w| w[1].level() < w[0].level());
                if plan.hops.len() > 1 && goes_up {
                    assert!(plan.tier >= Tier::R, "rule 1: {path:?} is {:?}", plan.tier);
                }
                // The tier is never better than the worst hop.
                assert!(plan.tier >= plan.hops.iter().map(|h| h.tier).max().unwrap());
            }
        }
    }
}

#[test]
fn rule_2_blocks_docx_through_pdf_to_md() {
    let planner = Planner::new(&registry(), &all(Platform::Linux));
    let plans = planner.plans(Docx, Md);
    assert!(!plans.is_empty());
    for p in &plans {
        assert!(p.hops.iter().all(|h| h.to != Pdf), "{p:?}");
    }
    assert_eq!(plans[0].hops.len(), 1, "the direct edge wins");
}

#[test]
fn reconstruction_beats_a_lossy_detour() {
    // DOCX -> MD directly is R; DOCX -> TXT -> MD has a lossy hop (L).
    let planner = Planner::new(&registry(), &all(Platform::Linux));
    let plans = planner.plans(Docx, Md);
    assert_eq!(plans[0].tier, Tier::R);
    assert_eq!(plans[0].hops[0].adapter, "pandoc-docs");
    let via_txt = plans
        .iter()
        .find(|p| p.hops[0].to == Txt)
        .expect("detour exists");
    assert_eq!(via_txt.tier, Tier::L);
}

#[test]
fn rule_1_caps_going_back_up() {
    // HEIC -> PNG -> PDF goes raster -> raster -> fixed: up after two hops.
    let planner = Planner::new(&registry(), &all(Platform::Linux));
    let via_png = planner
        .plans(Heic, Pdf)
        .into_iter()
        .find(|p| p.hops.len() == 2 && p.hops[0].to == Png)
        .expect("heic -> png -> pdf exists");
    assert_eq!(via_png.tier, Tier::R);
    assert_eq!(
        planner.best(Heic, Pdf).unwrap().tier,
        Tier::H,
        "direct edge wins"
    );
}

#[test]
fn ranking_prefers_lossless_over_cheap() {
    // PDF -> PNG -> WEBP (H, E) beats PDF -> WEBP (L) on worst tier,
    // even though it has more hops: rule 4 ranks lexicographically.
    let planner = Planner::new(&registry(), &all(Platform::Linux));
    let best = planner.best(Pdf, Webp).unwrap();
    assert_eq!(best.tier, Tier::H);
    assert_eq!(best.hops.len(), 2);
    // ...but the matrix fixes the offered cell as one direct, lossy hop.
    let cell = matrix::offered(Pdf, Webp).unwrap();
    assert_eq!(
        planner.best_for_cell(Pdf, Webp, cell).unwrap().tier,
        Tier::L
    );
}

#[test]
fn two_hop_cells_go_through_pdf() {
    let planner = Planner::new(&registry(), &all(Platform::Linux));
    let cell = matrix::offered(Pptx, Png).unwrap();
    let plan = planner.best_for_cell(Pptx, Png, cell).unwrap();
    let adapters: Vec<_> = plan.hops.iter().map(|h| h.adapter.as_str()).collect();
    assert_eq!(adapters, ["office-to-pdf", "pdf-to-image"]);
}

#[test]
fn plans_are_ordered_deterministically() {
    let planner = Planner::new(&registry(), &all(Platform::Linux));
    for _ in 0..3 {
        assert_eq!(planner.plans(Md, Txt), planner.plans(Md, Txt));
    }
}

// ------------------------------------------------------------- availability

fn direct(planner: &Planner, from: Format, to: Format) -> bool {
    planner.plans(from, to).iter().any(|p| p.hops.len() == 1)
}

#[test]
fn missing_engine_removes_its_edges() {
    let r = registry();
    let planner = Planner::new(&r, &without(&["libreoffice"]));
    assert!(!direct(&planner, Docx, Pdf));
    // The generic planner may still find DOCX -> MD -> PDF, but the offered
    // cell is one direct hop, so it isn't offered.
    let cell = matrix::offered(Docx, Pdf).unwrap();
    assert!(planner.best_for_cell(Docx, Pdf, cell).is_none());
    assert!(direct(&planner, Md, Pdf), "weasyprint is still installed");
}

#[test]
fn platform_filter() {
    let mut r = registry();
    for a in &mut r.adapters {
        if a.manifest.id == "office-to-pdf" {
            a.manifest.platforms = vec![Platform::Windows];
        }
    }
    assert!(!direct(&Planner::new(&r, &all(Platform::Linux)), Docx, Pdf));
    assert!(direct(
        &Planner::new(&r, &all(Platform::Windows)),
        Docx,
        Pdf
    ));
}

// ------------------------------------------------------------------ targets

#[test]
fn pdf_targets_include_operations_and_conversions() {
    let t = targets(&registry(), &all(Platform::Linux), &[Pdf, Pdf]);
    let merge = t
        .operations
        .iter()
        .find(|o| o.adapter == "pdf-merge")
        .unwrap();
    assert_eq!(merge.unavailable, None);
    let split = t
        .operations
        .iter()
        .find(|o| o.adapter == "pdf-split")
        .unwrap();
    assert_eq!(
        split.unavailable,
        Some(Unavailable::TooManyFiles { max: 1 })
    );

    let [group] = t.groups.as_slice() else {
        panic!("one group")
    };
    assert_eq!((group.format, group.files), (Pdf, 2));
    let to_txt = group.conversions.iter().find(|c| c.to == Txt).unwrap();
    assert!(to_txt.best_effort && to_txt.exact_with_text_layer);
    assert!(group
        .conversions
        .iter()
        .all(|c| c.to != Heic && c.to != Pdf));
    assert!(t.shared.is_empty());
}

#[test]
fn merge_needs_two_files() {
    let t = targets(&registry(), &all(Platform::Linux), &[Pdf]);
    let merge = t
        .operations
        .iter()
        .find(|o| o.adapter == "pdf-merge")
        .unwrap();
    assert_eq!(merge.unavailable, Some(Unavailable::TooFewFiles { min: 2 }));
}

#[test]
fn operations_need_every_file_to_qualify() {
    let t = targets(&registry(), &all(Platform::Linux), &[Pdf, Png]);
    assert!(t.operations.is_empty());
}

#[test]
fn mixed_selection_offers_shared_targets_at_the_worst_tier() {
    let t = targets(&registry(), &all(Platform::Linux), &[Png, Jpg, Heic]);
    assert_eq!(t.groups.len(), 3);
    let to_pdf = t.shared.iter().find(|c| c.to == Pdf).expect("all to PDF");
    assert_eq!(to_pdf.tier, Tier::H, "HEIC -> PDF is H, the others E");
    // PNG -> PNG isn't a conversion, so PNG isn't shared.
    assert!(t.shared.iter().all(|c| c.to != Png));
}

#[test]
fn missing_engine_is_reported_not_hidden() {
    let t = targets(&registry(), &without(&["libreoffice"]), &[Docx]);
    let to_pdf = t.groups[0]
        .conversions
        .iter()
        .find(|c| c.to == Pdf)
        .unwrap();
    assert_eq!(
        to_pdf.unavailable,
        Some(Unavailable::EngineMissing {
            engine: "libreoffice".into()
        })
    );
    assert_eq!(to_pdf.plan, None);
    let to_md = t.groups[0].conversions.iter().find(|c| c.to == Md).unwrap();
    assert_eq!(to_md.unavailable, None, "pandoc doesn't need LibreOffice");
}

#[test]
fn no_adapters_means_no_conversion_targets() {
    let empty = Registry {
        version: 1,
        adapters: vec![],
    };
    let t = targets(&empty, &all(Platform::Linux), &[Docx]);
    assert!(t.groups[0].conversions.is_empty());
}

#[test]
fn targets_serialize_for_the_ui() {
    let t = targets(&registry(), &without(&["libreoffice"]), &[Docx, Pdf]);
    let v = serde_json::to_value(&t).unwrap();
    assert!(v["groups"].is_array());
    let docx = v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["format"] == "docx")
        .unwrap();
    let pdf_target = docx["conversions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["to"] == "pdf")
        .unwrap();
    assert_eq!(pdf_target["unavailable"]["reason"], "engineMissing");
    assert_eq!(pdf_target["unavailable"]["engine"], "libreoffice");
    assert_eq!(pdf_target["bestEffort"], false);
}
