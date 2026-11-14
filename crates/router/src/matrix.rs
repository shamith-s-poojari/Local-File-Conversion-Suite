//! The compatibility matrix (blueprint section 3.2): which conversions
//! between the blueprint's 12 formats the product offers, and at what tier.
//! It's product policy, not derived from manifests: the planner only offers
//! a target the matrix lists, and CI checks that the manifests deliver every
//! listed cell at its tier.
//!
//! Formats added by ADR 0003 (legacy and OpenDocument office files, data
//! formats, more images, audio and video) aren't in the matrix: a pair that
//! involves one is offered when an adapter declares a direct edge for it.

use crate::model::{Format, Tier};

/// Copied from the blueprint. `.` is the same format, `ops` is PDF operations
/// (not conversions), `-` is deliberately not offered, `²` means two hops
/// through PDF, and `R*` is exact when the PDF has a text layer.
const MATRIX: &str = "
FROM \\ TO  PDF  DOCX PPTX XLSX MD   HTML TXT  PNG  JPG  WEBP SVG  HEIC
PDF        ops  R    R    R    R    R    R*   H    L    L    H    -
DOCX       H    .    -    -    R    R    L    H²   L²   L²   -    -
PPTX       H    -    .    -    -    -    L    H²   L²   L²   -    -
XLSX       H    -    -    .    -    R    L    H²   L²   L²   -    -
MD         H    H    R    -    .    H    L    -    -    -    -    -
HTML       H    R    -    -    R    .    L    -    -    -    -    -
TXT        H    H    -    -    H    H    .    -    -    -    -    -
PNG        E    -    -    -    -    -    -    .    L    E    -    -
JPG        E    -    -    -    -    -    -    E    .    L    -    -
WEBP       E    -    -    -    -    -    -    E    L    .    -    -
SVG        H    -    -    -    -    -    -    H    L    H    .    -
HEIC       H    -    -    -    -    -    -    H    L    L    -    .
";

/// The matrix's column order.
pub const BLUEPRINT: [Format; 12] = [
    Format::Pdf,
    Format::Docx,
    Format::Pptx,
    Format::Xlsx,
    Format::Md,
    Format::Html,
    Format::Txt,
    Format::Png,
    Format::Jpg,
    Format::Webp,
    Format::Svg,
    Format::Heic,
];

/// An offered conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub tier: Tier,
    /// 2 for the `²` cells, which go through PDF; otherwise 1.
    pub hops: u8,
    /// `R*`: exact when the PDF has a text layer, otherwise OCR is offered.
    pub exact_with_text_layer: bool,
}

/// The cell for `from → to`, or `None` if the matrix doesn't offer it.
pub fn offered(from: Format, to: Format) -> Option<Cell> {
    let row = MATRIX
        .lines()
        .map(str::split_whitespace)
        .find_map(|mut cols| (cols.next()? == from.as_str().to_uppercase()).then_some(cols))?;
    let index = BLUEPRINT.iter().position(|&f| f == to)?;
    parse_cell(row.into_iter().nth(index)?)
}

fn parse_cell(s: &str) -> Option<Cell> {
    let (body, hops) = match s.strip_suffix('²') {
        Some(b) => (b, 2),
        None => (s, 1),
    };
    let (body, exact_with_text_layer) = match body.strip_suffix('*') {
        Some(b) => (b, true),
        None => (body, false),
    };
    let tier = match body {
        "E" => Tier::E,
        "H" => Tier::H,
        "R" => Tier::R,
        "L" => Tier::L,
        _ => return None,
    };
    Some(Cell {
        tier,
        hops,
        exact_with_text_layer,
    })
}

/// Every offered cell.
pub fn all_offered() -> Vec<(Format, Format, Cell)> {
    let mut out = Vec::new();
    for from in BLUEPRINT {
        for to in BLUEPRINT {
            if let Some(cell) = offered(from, to) {
                out.push((from, to, cell));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_blueprint_matrix() {
        use Format::*;
        assert_eq!(offered(Pdf, Docx).unwrap().tier, Tier::R);
        assert!(offered(Pdf, Txt).unwrap().exact_with_text_layer);
        assert_eq!(
            offered(Docx, Png),
            Some(Cell {
                tier: Tier::H,
                hops: 2,
                exact_with_text_layer: false
            })
        );
        assert_eq!(offered(Pptx, Jpg).unwrap().hops, 2);
        assert_eq!(offered(Png, Pdf).unwrap().tier, Tier::E);
        assert_eq!(
            offered(Pdf, Pdf),
            None,
            "ops are operations, not conversions"
        );
        assert_eq!(offered(Docx, Docx), None);
        assert_eq!(offered(Png, Heic), None, "nothing converts to HEIC");
        assert_eq!(offered(Png, Svg), None, "raster to SVG is tracing");
    }

    #[test]
    fn every_row_has_12_cells() {
        let rows: Vec<_> = MATRIX
            .lines()
            .filter(|l| !l.trim().is_empty())
            .skip(1)
            .collect();
        assert_eq!(rows.len(), 12);
        for row in rows {
            assert_eq!(row.split_whitespace().count(), 13, "{row}");
        }
    }

    #[test]
    fn nothing_converts_to_heic() {
        for from in BLUEPRINT {
            assert_eq!(offered(from, Format::Heic), None);
        }
    }

    #[test]
    fn counts() {
        // Row by row: 10 + 7 + 5 + 6 + 5 + 4 + 4 + 3 + 3 + 3 + 4 + 4.
        assert_eq!(all_offered().len(), 58);
    }
}
