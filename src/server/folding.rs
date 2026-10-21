//! Line-based folds from the existing parse tree, including explicit regions.
//! Regions must also be returned here: clients may replace indentation/marker
//! folding entirely when a language server advertises folding support.
use lsp_types::{FoldingRange, FoldingRangeKind};
use tree_sitter::Tree;

use crate::tree_sitter_traversal::{Order, traverse};

pub(crate) fn folding_ranges(tree: &Tree, source: &str) -> Vec<FoldingRange> {
    let lines: Vec<_> = source.lines().collect();
    let mut ranges = Vec::new();
    let mut regions = Vec::new();
    for node in traverse(tree.walk(), Order::Pre) {
        let start = node.start_position().row;
        let mut end = node.end_position().row;
        let kind = match node.kind() {
            "line_comment" => {
                if !lines[start][..node.start_position().column]
                    .trim()
                    .is_empty()
                {
                    continue;
                }
                let text = source[node.byte_range()]
                    .strip_prefix("//")
                    .unwrap()
                    .trim_start();
                let marker = text.strip_prefix('#').unwrap_or(text);
                // Match the language configuration's region/endregion word boundary.
                let word = marker
                    .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .next();
                match word {
                    Some("region") => regions.push(start),
                    Some("endregion") => {
                        if let Some(open) = regions.pop() {
                            push_range(&mut ranges, open, start, Some(FoldingRangeKind::Region));
                        }
                    }
                    _ => {}
                }
                continue;
            }
            "block_comment" => Some(FoldingRangeKind::Comment),
            "module_item"
            | "function_item"
            | "union_block"
            | "transform_chain"
            | "if_block"
            | "for_block"
            | "intersection_for_block"
            | "let_block"
            | "assign_block"
            | "arguments"
            | "parameters"
            | "list"
            | "range"
            | "parenthesized_expression" => {
                // Leave a closing brace on its own line visible, as indentation
                // folding does. Do not hide the start of an else branch.
                if lines
                    .get(end)
                    .is_some_and(|line| line.trim_start().starts_with(['}', ']', ')']))
                {
                    end = end.saturating_sub(1);
                }
                None
            }
            _ => continue,
        };
        if node.end_position().column == 0 && end == node.end_position().row {
            end = end.saturating_sub(1);
        }
        push_range(&mut ranges, start, end, kind);
    }
    // A module and its opening block can start on the same line. Keep one
    // widest range per line so clients get stable, nested folding controls.
    ranges.sort_by(|a, b| {
        a.start_line
            .cmp(&b.start_line)
            .then(b.end_line.cmp(&a.end_line))
    });
    ranges.dedup_by_key(|range| range.start_line);
    ranges
}

fn push_range(
    ranges: &mut Vec<FoldingRange>,
    start: usize,
    end: usize,
    kind: Option<FoldingRangeKind>,
) {
    if end > start {
        ranges.push(FoldingRange {
            start_line: start as u32,
            end_line: end as u32,
            kind,
            ..Default::default()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folds(source: &str) -> Vec<FoldingRange> {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_openscad::LANGUAGE.into())
            .unwrap();
        let tree = parser.parse(source, None).unwrap();
        folding_ranges(&tree, source)
    }

    #[test]
    fn folds_unindented_modules_nested_geometry_functions_and_comments() {
        let source = "module shape() {\ndifference() {\ncube(10);\nsphere(4);\n}\n}\nfunction twice(x) =\nx * 2;\n/* notes\nmore notes\n*/\n";
        let ranges = folds(source);
        let spans: Vec<_> = ranges.iter().map(|r| (r.start_line, r.end_line)).collect();
        assert_eq!(spans, vec![(0, 4), (1, 3), (6, 7), (8, 10)]);
        assert_eq!(ranges.last().unwrap().kind, Some(FoldingRangeKind::Comment));
    }

    #[test]
    fn preserves_nested_regions_without_matching_strings_or_block_comments() {
        let source = "// #region Outer\n// region Inner\ncube(1);\n// endregion\n// #endregion\necho(\"// #region fake\");\n/*\n// #region fake\n*/\n// regional\n// endregion\ncube(1); // region inline\n/// region doc\n// region unclosed\n";
        let ranges = folds(source);
        let regions: Vec<_> = ranges
            .iter()
            .filter(|r| r.kind == Some(FoldingRangeKind::Region))
            .map(|r| (r.start_line, r.end_line))
            .collect();
        assert_eq!(regions, vec![(0, 4), (1, 3)]);
    }

    #[test]
    fn ignores_single_lines_and_handles_crlf_unicode_and_incomplete_code() {
        assert!(folds("module one() { cube(1); }\nfunction f(x) = x;\n").is_empty());
        let ranges =
            folds("// #region 中文\r\nmodule 部件() {\r\ncube(1);\r\n}\r\n// #endregion\r\n");
        assert_eq!((ranges[0].start_line, ranges[0].end_line), (0, 4));
        // Incomplete edits must not panic or emit empty/reversed ranges.
        assert!(
            folds("module shape() {\ncube(1);\n")
                .iter()
                .all(|r| r.end_line > r.start_line)
        );
    }

    #[test]
    fn folds_multiline_vectors_calls_and_unbraced_transform_chains() {
        let ranges = folds(
            "points = [\n[0, 0],\n[1, 1]\n];\ntranslate(\n[1, 2, 3]\n)\nrotate([0, 90, 0])\ncube(1);\n",
        );
        let spans: Vec<_> = ranges.iter().map(|r| (r.start_line, r.end_line)).collect();
        assert_eq!(spans, vec![(0, 2), (4, 8), (7, 8)]);
    }
}
