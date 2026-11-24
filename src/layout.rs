//! Rebuilding a tab's split layout in another workspace.
//!
//! herdr moves panes, not tabs. A tab moves by sending its first pane to a new
//! tab and then splitting the rest back in, one `pane.move` per pane, in an
//! order where every split lands on a pane that already covers its region.

use anyhow::{Context, Result, bail};
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Pane(String),
    Split {
        direction: String,
        /// The first child's share of the split.
        ratio: f64,
        first: Box<Node>,
        second: Box<Node>,
    },
}

/// One `pane.move`: `pane` goes beside `anchor`, or to the destination tab
/// when there is no anchor yet.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub pane: String,
    pub split: Option<Split>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Split {
    pub anchor: String,
    pub direction: String,
    pub ratio: f64,
}

impl Node {
    /// Reads the `root` of a `layout.export` result.
    pub fn from_export(node: &Value) -> Result<Self> {
        match node["type"].as_str() {
            Some("pane") => Ok(Self::Pane(
                node["pane_id"]
                    .as_str()
                    .context("layout pane has no pane_id")?
                    .to_owned(),
            )),
            Some("split") => Ok(Self::Split {
                direction: node["direction"]
                    .as_str()
                    .context("layout split has no direction")?
                    .to_owned(),
                ratio: node["ratio"].as_f64().unwrap_or(0.5),
                first: Box::new(Self::from_export(&node["first"])?),
                second: Box::new(Self::from_export(&node["second"])?),
            }),
            other => bail!("unknown layout node type {other:?}"),
        }
    }

    fn first_pane(&self) -> &str {
        match self {
            Self::Pane(id) => id,
            Self::Split { first, .. } => first.first_pane(),
        }
    }

    /// The moves that recreate this layout: the first pane alone, then each
    /// split as the first pane of its second half beside the first pane of
    /// its first half. Splitting a pane only divides that pane's own region,
    /// so working outside in reproduces every split.
    #[must_use]
    pub fn steps(&self) -> Vec<Step> {
        let mut steps = vec![Step {
            pane: self.first_pane().to_owned(),
            split: None,
        }];
        self.push_splits(&mut steps);
        steps
    }

    fn push_splits(&self, steps: &mut Vec<Step>) {
        if let Self::Split {
            direction,
            ratio,
            first,
            second,
        } = self
        {
            steps.push(Step {
                pane: second.first_pane().to_owned(),
                split: Some(Split {
                    anchor: first.first_pane().to_owned(),
                    direction: direction.clone(),
                    ratio: *ratio,
                }),
            });
            first.push_splits(steps);
            second.push_splits(steps);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn split(direction: &str, ratio: f64, first: &str, second: &str) -> Step {
        Step {
            pane: second.into(),
            split: Some(Split {
                anchor: first.into(),
                direction: direction.into(),
                ratio,
            }),
        }
    }

    #[test]
    fn single_pane_is_one_move() {
        let node = Node::from_export(&json!({ "type": "pane", "pane_id": "w1:p1" })).unwrap();
        assert_eq!(
            node.steps(),
            [Step {
                pane: "w1:p1".into(),
                split: None
            }]
        );
    }

    #[test]
    fn nested_splits_rebuild_outside_in() {
        // ┌────┬────┐
        // │ p1 │ p3 │
        // ├────┼────┤
        // │ p2 │ p4 │
        // └────┴────┘ with p4 split again to the right into p5.
        let pane = |id: &str| json!({ "type": "pane", "pane_id": id });
        let node = Node::from_export(&json!({
            "type": "split", "direction": "right", "ratio": 0.4,
            "first": { "type": "split", "direction": "down", "ratio": 0.5, "first": pane("p1"), "second": pane("p2") },
            "second": {
                "type": "split", "direction": "down", "ratio": 0.3, "first": pane("p3"),
                "second": { "type": "split", "direction": "right", "ratio": 0.6, "first": pane("p4"), "second": pane("p5") },
            },
        }))
        .unwrap();

        assert_eq!(
            node.steps(),
            [
                Step {
                    pane: "p1".into(),
                    split: None
                },
                split("right", 0.4, "p1", "p3"),
                split("down", 0.5, "p1", "p2"),
                split("down", 0.3, "p3", "p4"),
                split("right", 0.6, "p4", "p5"),
            ]
        );
    }
}
