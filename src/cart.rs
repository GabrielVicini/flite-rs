//! CART (classification and regression tree) interpretation.
//!
//! The duration, phrasing, part-of-speech, intonation and number-expansion
//! models are all binary decision trees over [feature paths](crate::ffeature).
//! A tree is stored as a flat array of nodes: the *yes* branch is always the
//! next node, so only the *no* branch needs an index. Leaves answer with a
//! constant.
//!
//! Feature paths are parsed when the tree is loaded rather than on every walk,
//! and each walk memoises the features it evaluates, since real trees ask the
//! same question along several branches.

use crate::data::{DataError, Reader};
use crate::ffeature::{self, FeaturePath};
use crate::utterance::{ItemId, Utterance};
use crate::value::Value;

const OP_IS: u8 = 0;
const OP_IN: u8 = 1;
const OP_LESS: u8 = 2;
const OP_GREATER: u8 = 3;
const OP_LEAF: u8 = 255;

#[derive(Debug)]
struct Node {
    feature: u8,
    op: u8,
    no_branch: u16,
    value: u16,
}

#[derive(Debug)]
pub struct Cart {
    features: Vec<FeaturePath>,
    values: Vec<Value>,
    nodes: Vec<Node>,
}

impl Cart {
    /// Decode one tree from its container section.
    pub fn parse(bytes: &[u8]) -> Result<Cart, DataError> {
        let mut r = Reader::new(bytes);
        let features = r
            .string_table()?
            .into_iter()
            .map(FeaturePath::parse)
            .collect();

        let value_count = r.u32()? as usize;
        let mut values = Vec::with_capacity(value_count);
        for _ in 0..value_count {
            values.push(match r.u8()? {
                0 => Value::str(r.short_str()?),
                _ => Value::Float(r.f32()?),
            });
        }

        let node_count = r.u32()? as usize;
        let mut nodes = Vec::with_capacity(node_count);
        for _ in 0..node_count {
            nodes.push(Node {
                feature: r.u8()?,
                op: r.u8()?,
                no_branch: r.u16()?,
                value: r.u16()?,
            });
        }
        if nodes.is_empty() {
            return Err(DataError("empty cart"));
        }
        Ok(Cart {
            features,
            values,
            nodes,
        })
    }

    /// Classify `item`, returning the leaf value.
    pub fn interpret(&self, utt: &Utterance, item: ItemId) -> &Value {
        // Small vector of (feature index, value): trees are shallow enough
        // that a linear scan beats a map, and this avoids re-walking relations
        // for a question asked twice on one path.
        let mut cache: Vec<(u8, Value)> = Vec::with_capacity(8);
        let mut node = 0usize;

        while self.nodes[node].op != OP_LEAF {
            let n = &self.nodes[node];
            let actual = match cache.iter().find(|(f, _)| *f == n.feature) {
                Some((_, v)) => v.clone(),
                None => {
                    let v = match self.features.get(n.feature as usize) {
                        Some(path) => ffeature::eval(utt, item, path),
                        None => Value::zero(),
                    };
                    cache.push((n.feature, v.clone()));
                    v
                }
            };
            let expected = &self.values[n.value as usize];
            let yes = match n.op {
                OP_IS => actual.equals(expected),
                OP_LESS => actual.less_than(expected),
                OP_GREATER => actual.greater_than(expected),
                // No shipped tree uses set membership or regex questions; if
                // one ever does, failing the question is the safe default.
                OP_IN => false,
                _ => false,
            };
            node = if yes { node + 1 } else { n.no_branch as usize };
        }
        &self.values[self.nodes[node].value as usize]
    }

    /// Classify `item` and return the leaf as a string.
    pub fn interpret_str(&self, utt: &Utterance, item: ItemId) -> &str {
        self.interpret(utt, item).as_str()
    }

    /// Classify `item` and return the leaf as a float.
    pub fn interpret_f32(&self, utt: &Utterance, item: ItemId) -> f32 {
        self.interpret(utt, item).as_f32()
    }
}
