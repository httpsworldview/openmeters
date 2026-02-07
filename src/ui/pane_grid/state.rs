// Simplified state management for the lightweight `pane_grid` widget.

use rustc_hash::FxHashMap;

use super::Pane;

// Ordered collection of panes that backs the lightweight [`PaneGrid`].
#[derive(Debug, Clone)]
pub struct State<T> {
    order: Vec<Pane>,
    panes: FxHashMap<Pane, T>,
    next_id: usize,
}

impl<T> State<T> {
    // Creates a new [`State`] with a single pane using the provided value.
    pub fn new(initial: T) -> (Self, Pane) {
        let pane = Pane(0);
        let mut panes = FxHashMap::default();
        panes.insert(pane, initial);

        (
            Self {
                order: vec![pane],
                panes,
                next_id: 1,
            },
            pane,
        )
    }

    // Returns the number of panes tracked by the [`State`].
    pub fn len(&self) -> usize {
        self.order.len()
    }

    // Returns `true` when the [`State`] contains no panes.
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    // Returns the value associated with the given [`Pane`], if any.
    pub fn get(&self, pane: Pane) -> Option<&T> {
        self.panes.get(&pane)
    }

    // Returns a mutable reference to the value associated with the given [`Pane`], if any.
    pub fn get_mut(&mut self, pane: Pane) -> Option<&mut T> {
        self.panes.get_mut(&pane)
    }

    // Returns an iterator over the panes in their visual order.
    pub fn iter(&self) -> impl Iterator<Item = (&Pane, &T)> {
        self.order
            .iter()
            .map(move |pane| (pane, self.panes.get(pane).expect("missing pane state")))
    }

    // Inserts a new pane immediately to the right of `pane` and returns its identifier.
    pub fn insert_after(&mut self, pane: Pane, state: T) -> Option<Pane> {
        let index = self.position(pane)?;
        let new_pane = Pane(self.next_id);
        self.next_id += 1;

        self.order.insert(index + 1, new_pane);
        self.panes.insert(new_pane, state);

        Some(new_pane)
    }

    // Moves pane `a` to the position of pane `b`, shifting intermediate panes.
    pub fn move_to(&mut self, a: Pane, b: Pane) -> bool {
        let (Some(from), Some(to)) = (self.position(a), self.position(b)) else {
            return false;
        };
        if from == to {
            return false;
        }
        let pane = self.order.remove(from);
        self.order.insert(to, pane);
        true
    }

    // Applies `f` to each pane value in visual order.
    pub fn for_each_mut(&mut self, mut f: impl FnMut(Pane, &mut T)) {
        let order = self.order.clone();

        for pane in order {
            if let Some(value) = self.panes.get_mut(&pane) {
                f(pane, value);
            }
        }
    }

    fn position(&self, pane: Pane) -> Option<usize> {
        self.order.iter().position(|id| *id == pane)
    }
}
