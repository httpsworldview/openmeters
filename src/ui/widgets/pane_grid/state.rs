use std::collections::HashMap;

use super::Pane;

#[derive(Debug, Clone)]
pub struct State<T> {
    order: Vec<Pane>,
    panes: HashMap<Pane, T>,
    next_id: usize,
}

impl<T> State<T> {
    pub fn new(initial: T) -> (Self, Pane) {
        let pane = Pane(0);
        let mut panes = HashMap::default();
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

    pub fn get(&self, pane: Pane) -> Option<&T> {
        self.panes.get(&pane)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Pane, &T)> {
        self.order
            .iter()
            .filter_map(move |pane| self.panes.get(pane).map(|state| (pane, state)))
    }

    pub fn insert_after(&mut self, pane: Pane, state: T) -> Option<Pane> {
        let index = self.position(pane)?;
        let new_pane = Pane(self.next_id);
        self.next_id += 1;

        self.order.insert(index + 1, new_pane);
        self.panes.insert(new_pane, state);

        Some(new_pane)
    }

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

    pub fn for_each_mut(&mut self, mut f: impl FnMut(Pane, &mut T)) {
        for i in 0..self.order.len() {
            let pane = self.order[i];
            if let Some(value) = self.panes.get_mut(&pane) {
                f(pane, value);
            }
        }
    }

    fn position(&self, pane: Pane) -> Option<usize> {
        self.order.iter().position(|id| *id == pane)
    }
}
