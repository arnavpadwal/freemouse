use crate::layout::WorkspaceLayout;
use crate::network::Edge;
use uuid::Uuid;

pub struct EdgeRouter {
    pub local_machine_id: Uuid,
    pub layout: WorkspaceLayout,
}

impl EdgeRouter {
    pub fn new(local_machine_id: Uuid, layout: WorkspaceLayout) -> Self {
        Self {
            local_machine_id,
            layout,
        }
    }

    pub fn update_layout(&mut self, layout: WorkspaceLayout) {
        self.layout = layout;
    }

    /// Returns (neighbor_id, exit_edge, y_ratio) when cursor crosses an edge with a neighbor.
    pub fn check_edge_cross(
        &self,
        x: f64,
        y: f64,
        screen_width: f64,
        screen_height: f64,
        threshold: f64,
    ) -> Option<(Uuid, Edge, f64)> {
        let y_ratio = if screen_height > 0.0 {
            (y / screen_height).clamp(0.0, 1.0)
        } else {
            0.5
        };

        if x >= screen_width - threshold {
            if let Some(n) = self.layout.neighbor_at_edge(self.local_machine_id, Edge::Right) {
                return Some((n.machine_id, Edge::Right, y_ratio));
            }
        }
        if x <= threshold {
            if let Some(n) = self.layout.neighbor_at_edge(self.local_machine_id, Edge::Left) {
                return Some((n.machine_id, Edge::Left, y_ratio));
            }
        }
        if y >= screen_height - threshold {
            if let Some(n) = self.layout.neighbor_at_edge(self.local_machine_id, Edge::Bottom) {
                return Some((n.machine_id, Edge::Bottom, y_ratio));
            }
        }
        if y <= threshold {
            if let Some(n) = self.layout.neighbor_at_edge(self.local_machine_id, Edge::Top) {
                return Some((n.machine_id, Edge::Top, y_ratio));
            }
        }
        None
    }

    /// Warp position on target machine when entering from an edge.
    pub fn entry_position(
        from_edge: Edge,
        y_ratio: f64,
        screen_width: f64,
        screen_height: f64,
    ) -> (f64, f64) {
        let y = y_ratio * screen_height;
        match from_edge {
            Edge::Left => (1.0, y),
            Edge::Right => (screen_width - 1.0, y),
            Edge::Top => (screen_width / 2.0, 1.0),
            Edge::Bottom => (screen_width / 2.0, screen_height - 1.0),
        }
    }

    pub fn exit_position(
        to_edge: Edge,
        y_ratio: f64,
        screen_width: f64,
        screen_height: f64,
    ) -> (f64, f64) {
        let y = y_ratio * screen_height;
        match to_edge {
            Edge::Left => (1.0, y),
            Edge::Right => (screen_width - 1.0, y),
            Edge::Top => (screen_width / 2.0, 1.0),
            Edge::Bottom => (screen_width / 2.0, screen_height - 1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::MachineLayout;
    use crate::network::ScreenInfo;

    #[test]
    fn y_ratio_mapping() {
        let (x, y) = EdgeRouter::entry_position(Edge::Left, 0.5, 1920.0, 1080.0);
        assert!((x - 1.0).abs() < 1e-6);
        assert!((y - 540.0).abs() < 1e-6);
    }

    #[test]
    fn edge_cross_detects_right() {
        let id = Uuid::new_v4();
        let neighbor_id = Uuid::new_v4();
        let layout = WorkspaceLayout {
            machines: vec![
                MachineLayout {
                    machine_id: id,
                    hostname: "A".into(),
                    ip: "1".into(),
                    grid_pos: (0, 0),
                    screens: vec![ScreenInfo {
                        x: 0.0,
                        y: 0.0,
                        width: 1920.0,
                        height: 1080.0,
                        primary: true,
                    }],
                },
                MachineLayout {
                    machine_id: neighbor_id,
                    hostname: "B".into(),
                    ip: "2".into(),
                    grid_pos: (1, 0),
                    screens: vec![ScreenInfo {
                        x: 0.0,
                        y: 0.0,
                        width: 1920.0,
                        height: 1080.0,
                        primary: true,
                    }],
                },
            ],
        };
        let router = EdgeRouter::new(id, layout);
        let result = router.check_edge_cross(1919.0, 540.0, 1920.0, 1080.0, 2.0);
        assert!(result.is_some());
        let (nid, edge, ratio) = result.unwrap();
        assert_eq!(nid, neighbor_id);
        assert_eq!(edge, Edge::Right);
        assert!((ratio - 0.5).abs() < 0.01);
    }
}
