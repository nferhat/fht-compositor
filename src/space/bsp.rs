use smithay::utils::{Logical, Rectangle};

#[derive(Debug)]
pub enum Split {
    Horizontal,
    Vertical
}

/// A BSP Tree Node can either have zero or two children.
#[derive(Debug)]
pub struct BspNode {
    pub rect: Rectangle<i32, Logical>,
    pub split: Split,
    pub first_child: Option<usize>,
    pub second_child: Option<usize>,
    pub parent: Option<usize>,
}

#[derive(Debug)]
pub struct BspTree {
    pub arena: Vec<BspNode>,
    pub leaves: usize,
}

impl BspNode {
    pub fn new(rect: Rectangle<i32, Logical>, split: Split, parent: Option<usize>) -> BspNode {
        BspNode {
            rect,
            split,
            first_child: None,
            second_child: None,
            parent
        }
    }

    pub fn is_leaf(&self) -> bool {
        self.first_child.is_none() && self.second_child.is_none()
    }
}

impl BspTree {
    pub fn new(rect: Rectangle<i32, Logical>, len: usize) -> Self {
        let root = BspNode::new(rect, Split::Horizontal, None);

        let mut arena = Vec::with_capacity(len);
        arena.push(root);

        BspTree {
            arena,
            leaves: 1,
        }
    }

    pub fn add_child(&mut self, node: BspNode) -> usize {
        let idx = self.arena.len();
        if let Some(parent) = node.parent {
            if self.arena[parent].first_child.is_none() {
                self.arena[parent].first_child = Some(idx);
            } else if self.arena[parent].second_child.is_none() {
                self.arena[parent].second_child = Some(idx);
            }
        }
        self.arena.push(node);

        idx
    }
}

pub fn build_tree(tree: &mut BspTree, idx: usize, len: usize, split_ratio: f64, inner_gaps: i32) {
    if tree.arena[idx].is_leaf() && tree.leaves < len {
        let mut first_rect = tree.arena[idx].rect;
        let mut second_rect = tree.arena[idx].rect;
        let mut first_split = Split::Vertical;
        let mut second_split = Split::Vertical;

        match tree.arena[idx].split {
            Split::Horizontal => {
                let nh = (tree.arena[idx].rect.size.h as f64 * split_ratio) as i32 - (inner_gaps / 2);
                let nly = tree.arena[idx].rect.loc.y + nh + inner_gaps;
                first_rect.size = (first_rect.size.w, nh).into();
                second_rect.size = (second_rect.size.w, nh).into();
                second_rect.loc = (second_rect.loc.x, nly).into();
            }
            Split::Vertical => {
                let nw = (tree.arena[idx].rect.size.w as f64 * split_ratio) as i32 - (inner_gaps / 2);
                let nlx = tree.arena[idx].rect.loc.x + nw + inner_gaps;
                first_rect.size = (nw, first_rect.size.h).into();
                second_rect.size = (nw, second_rect.size.h).into();
                second_rect.loc = (nlx, second_rect.loc.y).into();

                first_split = Split::Horizontal;
                second_split = Split::Horizontal;
            }
        }
        
        let first_child = BspNode::new(first_rect, first_split, Some(idx));
        let second_child = BspNode::new(second_rect, second_split, Some(idx));

        let _ = tree.add_child(first_child);
        let second_idx = tree.add_child(second_child);

        // We're technically adding two leaves, but then we iterate into another branch in the
        // `build_tree` call, so it's plus two, minus one
        tree.leaves += 1;

        build_tree(tree, second_idx, len, split_ratio, inner_gaps); 
    } else {
        return;
    }
}

pub fn to_leaves(tree: &mut BspTree, root: usize, leaves: &mut Vec<Rectangle<i32, Logical>>) {
    if tree.arena[root].is_leaf() {
        leaves.push(tree.arena[root].rect);
    } else {
        if let Some(first) = tree.arena[root].first_child {
            to_leaves(tree, first, leaves);
        }
        if let Some(second) = tree.arena[root].second_child {
            to_leaves(tree, second, leaves);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correct_sizes() {
        let mut tree = BspTree::new(Rectangle::new((0, 0).into(), (100, 100).into()), 4);
        let mut leaves = Vec::new();
        build_tree(&mut tree, 0, 4, 0.5, 0);
        to_leaves(&mut tree, 0, &mut leaves);

        assert_eq!(leaves[0].size.w, 100);
        assert_eq!(leaves[0].size.h, 50);

        assert_eq!(leaves[1].size.w, 50);
        assert_eq!(leaves[1].size.h, 50);

        assert_eq!(leaves[2].size.w, 50);
        assert_eq!(leaves[2].size.h, 25);

        assert_eq!(leaves[3].size.w, 50);
        assert_eq!(leaves[3].size.h, 25);
        
        assert_eq!(leaves[0].loc.x, 0);
        assert_eq!(leaves[0].loc.y, 0);

        assert_eq!(leaves[1].loc.x, 0);
        assert_eq!(leaves[1].loc.y, 50);

        assert_eq!(leaves[2].loc.x, 50);
        assert_eq!(leaves[2].loc.y, 50);

        assert_eq!(leaves[3].loc.x, 50);
        assert_eq!(leaves[3].loc.y, 75);
    }

    #[test]
    fn correct_sizes_with_gaps() {
        let mut tree = BspTree::new(Rectangle::new((0, 0).into(), (100, 100).into()), 4);
        let mut leaves = Vec::new();
        build_tree(&mut tree, 0, 4, 0.5, 4);
        to_leaves(&mut tree, 0, &mut leaves);

        dbg!(&tree);

        assert_eq!(leaves[0].size.w, 100);
        assert_eq!(leaves[0].size.h, 48);
        
        assert_eq!(leaves[1].size.w, 48);
        assert_eq!(leaves[1].size.h, 48);
        
        assert_eq!(leaves[2].size.w, 48);
        assert_eq!(leaves[2].size.h, 22);
        
        assert_eq!(leaves[3].size.w, 48);
        assert_eq!(leaves[3].size.h, 22);
        
        assert_eq!(leaves[0].loc.x, 0);
        assert_eq!(leaves[0].loc.y, 0);
        
        assert_eq!(leaves[1].loc.x, 0);
        assert_eq!(leaves[1].loc.y, 52);
        
        assert_eq!(leaves[2].loc.x, 52);
        assert_eq!(leaves[2].loc.y, 52);
        
        assert_eq!(leaves[3].loc.x, 52);
        assert_eq!(leaves[3].loc.y, 78);
    }
}
