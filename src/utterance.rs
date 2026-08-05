//! The heterogeneous relation graph (HRG) that carries an utterance through
//! the pipeline.
//!
//! An utterance is a set of named *relations* (Token, Word, Syllable, Segment
//! and so on) over a shared pool of *items*. The same linguistic object appears in
//! several relations at once: a word is a node in the flat `Word` chain and
//! simultaneously the root of a tree in `SylStructure`. Both nodes share one
//! feature set, so setting `pos` on the word is visible from either view.
//!
//! # Representation
//!
//! Everything lives in flat arenas indexed by [`ItemId`] and [`RelId`], so the
//! graph is plain data with no reference counting or interior mutability, and
//! the whole structure drops in one pass. An [`ItemId`] names a *node*, i.e. a
//! (content, relation) pair; [`Utterance::item_as`] moves between the views of
//! one content, and [`Utterance::same_item`] tests whether two nodes are views
//! of the same thing.
//!
//! Ids are only meaningful for the utterance that produced them. Nodes are
//! never removed, so an id stays valid for the utterance's lifetime.

use crate::value::{Features, Value};

/// A node in one relation. See the module docs on identity.
pub type ItemId = usize;
/// A relation within an utterance.
pub type RelId = usize;

/// Feature set shared by every relation-view of one linguistic object.
#[derive(Debug)]
struct Contents {
    features: Features,
    /// Which node represents this content in each relation it appears in.
    views: Vec<(RelId, ItemId)>,
}

#[derive(Debug)]
struct Node {
    contents: usize,
    relation: RelId,
    prev: Option<ItemId>,
    next: Option<ItemId>,
    parent: Option<ItemId>,
    first_daughter: Option<ItemId>,
    last_daughter: Option<ItemId>,
}

#[derive(Debug)]
struct Relation {
    name: Box<str>,
    head: Option<ItemId>,
    tail: Option<ItemId>,
}

#[derive(Debug, Default)]
pub struct Utterance {
    contents: Vec<Contents>,
    nodes: Vec<Node>,
    relations: Vec<Relation>,
    /// Utterance-scoped parameters (`duration_stretch`, `f0_shift` and so on).
    pub features: Features,
}

impl Utterance {
    pub fn new() -> Utterance {
        Utterance::default()
    }

    /// Create a relation, replacing any existing one of the same name.
    pub fn create_relation(&mut self, name: &str) -> RelId {
        if let Some(id) = self.relation(name) {
            self.relations[id].head = None;
            self.relations[id].tail = None;
            return id;
        }
        self.relations.push(Relation {
            name: Box::from(name),
            head: None,
            tail: None,
        });
        self.relations.len() - 1
    }

    pub fn relation(&self, name: &str) -> Option<RelId> {
        self.relations.iter().position(|r| &*r.name == name)
    }

    pub fn head(&self, rel: RelId) -> Option<ItemId> {
        self.relations[rel].head
    }

    pub fn tail(&self, rel: RelId) -> Option<ItemId> {
        self.relations[rel].tail
    }

    /// Head of a relation looked up by name; `None` if the relation is absent.
    pub fn head_of(&self, name: &str) -> Option<ItemId> {
        self.relation(name).and_then(|r| self.head(r))
    }

    /// Iterate the top-level chain of a relation.
    pub fn iter_relation(&self, name: &str) -> ItemIter<'_> {
        ItemIter {
            utt: self,
            next: self.head_of(name),
        }
    }

    /// Iterate the sibling chain starting at `item` (inclusive).
    pub fn iter_from(&self, item: Option<ItemId>) -> ItemIter<'_> {
        ItemIter {
            utt: self,
            next: item,
        }
    }

    fn new_node(&mut self, contents: usize, relation: RelId) -> ItemId {
        let id = self.nodes.len();
        self.nodes.push(Node {
            contents,
            relation,
            prev: None,
            next: None,
            parent: None,
            first_daughter: None,
            last_daughter: None,
        });
        self.contents[contents].views.push((relation, id));
        id
    }

    /// Contents for a new node: either fresh, or shared with `share`.
    fn contents_for(&mut self, share: Option<ItemId>) -> usize {
        match share {
            Some(item) => self.nodes[item].contents,
            None => {
                self.contents.push(Contents {
                    features: Features::new(),
                    views: Vec::new(),
                });
                self.contents.len() - 1
            }
        }
    }

    /// Append to a relation's top-level chain.
    ///
    /// Passing `share` links an existing item into this relation as well,
    /// which is how a word enters `SylStructure` while staying in `Word`.
    pub fn append(&mut self, rel: RelId, share: Option<ItemId>) -> ItemId {
        let contents = self.contents_for(share);
        let id = self.new_node(contents, rel);
        match self.relations[rel].tail {
            Some(tail) => {
                self.nodes[tail].next = Some(id);
                self.nodes[id].prev = Some(tail);
            }
            None => self.relations[rel].head = Some(id),
        }
        self.relations[rel].tail = Some(id);
        id
    }

    /// Add a daughter to `parent`, in the parent's own relation.
    pub fn add_daughter(&mut self, parent: ItemId, share: Option<ItemId>) -> ItemId {
        let rel = self.nodes[parent].relation;
        let contents = self.contents_for(share);
        let id = self.new_node(contents, rel);
        self.nodes[id].parent = Some(parent);
        match self.nodes[parent].last_daughter {
            Some(last) => {
                self.nodes[last].next = Some(id);
                self.nodes[id].prev = Some(last);
            }
            None => self.nodes[parent].first_daughter = Some(id),
        }
        self.nodes[parent].last_daughter = Some(id);
        id
    }

    /// Insert a new node directly before `item` in its sibling chain.
    pub fn insert_before(&mut self, item: ItemId, share: Option<ItemId>) -> ItemId {
        let rel = self.nodes[item].relation;
        let contents = self.contents_for(share);
        let id = self.new_node(contents, rel);
        let prev = self.nodes[item].prev;
        self.nodes[id].prev = prev;
        self.nodes[id].next = Some(item);
        self.nodes[id].parent = self.nodes[item].parent;
        self.nodes[item].prev = Some(id);
        match prev {
            Some(p) => self.nodes[p].next = Some(id),
            None => match self.nodes[id].parent {
                Some(parent) => self.nodes[parent].first_daughter = Some(id),
                None => self.relations[rel].head = Some(id),
            },
        }
        id
    }

    /// Insert a new node directly after `item` in its sibling chain.
    pub fn insert_after(&mut self, item: ItemId, share: Option<ItemId>) -> ItemId {
        let rel = self.nodes[item].relation;
        let contents = self.contents_for(share);
        let id = self.new_node(contents, rel);
        let next = self.nodes[item].next;
        self.nodes[id].next = next;
        self.nodes[id].prev = Some(item);
        self.nodes[id].parent = self.nodes[item].parent;
        self.nodes[item].next = Some(id);
        match next {
            Some(n) => self.nodes[n].prev = Some(id),
            None => match self.nodes[id].parent {
                Some(parent) => self.nodes[parent].last_daughter = Some(id),
                None => self.relations[rel].tail = Some(id),
            },
        }
        id
    }

    pub fn next(&self, item: ItemId) -> Option<ItemId> {
        self.nodes[item].next
    }

    pub fn prev(&self, item: ItemId) -> Option<ItemId> {
        self.nodes[item].prev
    }

    pub fn parent(&self, item: ItemId) -> Option<ItemId> {
        self.nodes[item].parent
    }

    pub fn daughter(&self, item: ItemId) -> Option<ItemId> {
        self.nodes[item].first_daughter
    }

    pub fn last_daughter(&self, item: ItemId) -> Option<ItemId> {
        self.nodes[item].last_daughter
    }

    /// First item in `item`'s sibling chain.
    pub fn first(&self, item: ItemId) -> ItemId {
        let mut cur = item;
        while let Some(p) = self.nodes[cur].prev {
            cur = p;
        }
        cur
    }

    /// Last item in `item`'s sibling chain.
    pub fn last(&self, item: ItemId) -> ItemId {
        let mut cur = item;
        while let Some(n) = self.nodes[cur].next {
            cur = n;
        }
        cur
    }

    /// This item viewed in another relation, if it participates in one.
    pub fn item_as(&self, item: ItemId, relation: &str) -> Option<ItemId> {
        let rel = self.relation(relation)?;
        self.contents[self.nodes[item].contents]
            .views
            .iter()
            .find(|(r, _)| *r == rel)
            .map(|(_, id)| *id)
    }

    /// Whether two nodes are views of the same linguistic object.
    pub fn same_item(&self, a: ItemId, b: ItemId) -> bool {
        self.nodes[a].contents == self.nodes[b].contents
    }

    pub fn feature(&self, item: ItemId, name: &str) -> Option<&Value> {
        self.contents[self.nodes[item].contents].features.get(name)
    }

    pub fn set_feature(&mut self, item: ItemId, name: &str, value: Value) {
        self.contents[self.nodes[item].contents]
            .features
            .set(name, value);
    }

    pub fn set_str(&mut self, item: ItemId, name: &str, value: &str) {
        self.set_feature(item, name, Value::str(value));
    }

    pub fn has_feature(&self, item: ItemId, name: &str) -> bool {
        self.feature(item, name).is_some()
    }

    pub fn remove_feature(&mut self, item: ItemId, name: &str) {
        let c = self.nodes[item].contents;
        self.contents[c].features.remove(name);
    }

    /// The item's `name` feature, or `""`.
    pub fn name(&self, item: ItemId) -> &str {
        self.feature(item, "name").map_or("", |v| v.as_str())
    }

    pub fn feature_str(&self, item: ItemId, name: &str) -> &str {
        self.feature(item, name).map_or("", |v| v.as_str())
    }

    pub fn feature_f32(&self, item: ItemId, name: &str) -> f32 {
        self.feature(item, name).map_or(0.0, |v| v.as_f32())
    }

    pub fn feature_i32(&self, item: ItemId, name: &str) -> i32 {
        self.feature(item, name).map_or(0, |v| v.as_i32())
    }
}

/// Iterator over a sibling chain.
pub struct ItemIter<'a> {
    utt: &'a Utterance,
    next: Option<ItemId>,
}

impl Iterator for ItemIter<'_> {
    type Item = ItemId;

    fn next(&mut self) -> Option<ItemId> {
        let cur = self.next?;
        self.next = self.utt.next(cur);
        Some(cur)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_contents_are_visible_from_both_relations() {
        let mut u = Utterance::new();
        let words = u.create_relation("Word");
        let syls = u.create_relation("SylStructure");
        let w = u.append(words, None);
        u.set_str(w, "name", "hello");
        let sw = u.append(syls, Some(w));

        assert_eq!(u.name(sw), "hello");
        u.set_str(sw, "pos", "nn");
        assert_eq!(u.feature_str(w, "pos"), "nn");
        assert_eq!(u.item_as(w, "SylStructure"), Some(sw));
        assert!(u.same_item(w, sw));
    }

    #[test]
    fn insert_before_fixes_up_relation_head() {
        let mut u = Utterance::new();
        let seg = u.create_relation("Segment");
        let a = u.append(seg, None);
        let b = u.insert_before(a, None);
        assert_eq!(u.head(seg), Some(b));
        assert_eq!(u.next(b), Some(a));
        assert_eq!(u.prev(a), Some(b));
    }

    #[test]
    fn insert_before_fixes_up_daughter_list() {
        let mut u = Utterance::new();
        let rel = u.create_relation("SylStructure");
        let parent = u.append(rel, None);
        let d = u.add_daughter(parent, None);
        let d0 = u.insert_before(d, None);
        assert_eq!(u.daughter(parent), Some(d0));
        assert_eq!(u.parent(d0), Some(parent));
        assert_eq!(u.last_daughter(parent), Some(d));
    }
}
