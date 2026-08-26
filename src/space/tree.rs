use fht_compositor_config::WorkspaceLayout;
use smithay::utils::{Logical, Rectangle};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Split {
    Horizontal,
    Vertical,
}

/// A BSP Tree Node can either have zero or two children.
#[derive(Debug)]
pub struct Node {
    pub rect: Rectangle<i32, Logical>,
    pub split: Split,
    pub first_child: Option<usize>,
    pub second_child: Option<usize>,
    pub parent: Option<usize>,
}

#[derive(Debug)]
pub struct Tree {
    pub arena: Vec<Node>,
    pub leaves: usize,
    layout: WorkspaceLayout,
    inner_gaps: i32,
}

impl Node {
    pub fn new(rect: Rectangle<i32, Logical>, split: Split, parent: Option<usize>) -> Node {
        Node {
            rect,
            split,
            first_child: None,
            second_child: None,
            parent,
        }
    }

    pub fn is_leaf(&self) -> bool {
        self.first_child.is_none() && self.second_child.is_none()
    }
}

impl Tree {
    pub fn new(
        layout: WorkspaceLayout,
        rect: Rectangle<i32, Logical>,
        len: usize,
        inner_gaps: i32,
    ) -> Self {
        let root = Node::new(rect, Split::Horizontal, None);

        let capacity = len
            .checked_mul(2)
            .and_then(|value| value.checked_sub(1))
            .unwrap_or(1);

        let mut arena = Vec::with_capacity(capacity);
        arena.push(root);

        Tree {
            arena,
            leaves: 1,
            layout,
            inner_gaps,
        }
    }

    pub fn add_child(&mut self, node: Node) -> usize {
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

    pub fn grow(&mut self, mut idx: usize, target_leaves: usize, split_ratio: f64) {
        while self.leaves < target_leaves {
            if !self.arena[idx].is_leaf() {
                break;
            }

            idx = self.split_leaf(idx, split_ratio);
            self.leaves += 1;
        }
    }

    fn split_leaf(&mut self, idx: usize, split_ratio: f64) -> usize {
        let mut first_rect = self.arena[idx].rect;
        let mut second_rect = self.arena[idx].rect;

        let child_split = match self.arena[idx].split {
            Split::Horizontal => {
                let usable = self.arena[idx].rect.size.h - self.inner_gaps;
                let first_h = (usable as f64 * split_ratio).round() as i32;
                let second_h = usable - first_h;

                first_rect.size.h = first_h;
                second_rect.size.h = second_h;

                if self.leaves % 4 == 3 && self.layout == WorkspaceLayout::SpiralTree {
                    first_rect.loc.y = self.arena[idx].rect.loc.y + second_h + self.inner_gaps;
                } else {
                    second_rect.loc.y = self.arena[idx].rect.loc.y + first_h + self.inner_gaps;
                }

                Split::Vertical
            }
            Split::Vertical => {
                let usable = self.arena[idx].rect.size.w - self.inner_gaps;
                let first_w = (usable as f64 * split_ratio).round() as i32;
                let second_w = usable - first_w;

                first_rect.size.w = first_w;
                second_rect.size.w = second_w;

                if self.leaves % 4 == 2 && self.layout == WorkspaceLayout::SpiralTree {
                    first_rect.loc.x = self.arena[idx].rect.loc.x + second_w + self.inner_gaps;
                } else {
                    second_rect.loc.x = self.arena[idx].rect.loc.x + first_w + self.inner_gaps;
                }

                Split::Horizontal
            }
        };

        let first_child = Node::new(first_rect, child_split, Some(idx));
        let second_child = Node::new(second_rect, child_split, Some(idx));

        self.add_child(first_child);
        self.add_child(second_child)
    }

    pub fn leaf_rects(&self, root: usize) -> Vec<Rectangle<i32, Logical>> {
        let mut leaves = Vec::new();
        let mut pending = vec![root];

        while let Some(idx) = pending.pop() {
            let node = &self.arena[idx];

            if node.is_leaf() {
                leaves.push(node.rect);
                continue;
            }

            // LIFO stack, we push the second child first so that one pops
            if let Some(second) = node.second_child {
                pending.push(second);
            }

            if let Some(first) = node.first_child {
                pending.push(first);
            }
        }

        leaves
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correct_sizes() {
        let mut tree = Tree::new(
            WorkspaceLayout::BinaryTree,
            Rectangle::new((0, 0).into(), (100, 100).into()),
            4,
            0,
        );
        tree.grow(0, 4, 0.5);
        let leaves = tree.leaf_rects(0);

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
    fn correct_locs_for_spiral() {
        let mut tree = Tree::new(
            WorkspaceLayout::SpiralTree,
            Rectangle::new((0, 0).into(), (100, 100).into()),
            4,
            0,
        );
        tree.grow(0, 5, 0.5);
        let leaves = tree.leaf_rects(0);

        assert_eq!(leaves[0].loc.x, 0);
        assert_eq!(leaves[0].loc.y, 0);

        assert_eq!(leaves[1].loc.x, 50);
        assert_eq!(leaves[1].loc.y, 50);

        assert_eq!(leaves[2].loc.x, 0);
        assert_eq!(leaves[2].loc.y, 75);

        assert_eq!(leaves[3].loc.x, 0);
        assert_eq!(leaves[3].loc.y, 50);

        assert_eq!(leaves[4].loc.x, 25);
        assert_eq!(leaves[4].loc.y, 50);
    }

    #[test]
    fn correct_sizes_with_gaps() {
        let mut tree = Tree::new(
            WorkspaceLayout::BinaryTree,
            Rectangle::new((0, 0).into(), (100, 100).into()),
            4,
            4,
        );
        tree.grow(0, 4, 0.5);
        let leaves = tree.leaf_rects(0);

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
