use crate::network::{Edge, ScreenInfo};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

pub const MAX_MACHINES: usize = 4;
pub const PROTOCOL_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MachineLayout {
    pub machine_id: Uuid,
    pub hostname: String,
    pub ip: String,
    pub grid_pos: (u8, u8),
    pub screens: Vec<ScreenInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct WorkspaceLayout {
    pub machines: Vec<MachineLayout>,
}

impl WorkspaceLayout {
    pub fn load() -> Self {
        config_path()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        if let Some(path) = config_path() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let content = toml::to_string_pretty(self).unwrap_or_default();
            fs::write(path, content)?;
        }
        Ok(())
    }

    pub fn add_or_update_machine(&mut self, machine: MachineLayout) {
        if let Some(existing) = self
            .machines
            .iter_mut()
            .find(|m| m.machine_id == machine.machine_id)
        {
            *existing = machine;
        } else if self.machines.len() < MAX_MACHINES {
            self.machines.push(machine);
        }
    }

    pub fn remove_machine(&mut self, machine_id: Uuid) {
        self.machines.retain(|m| m.machine_id != machine_id);
    }

    pub fn neighbor_at_edge(&self, machine_id: Uuid, edge: Edge) -> Option<&MachineLayout> {
        let me = self.machines.iter().find(|m| m.machine_id == machine_id)?;
        let (gx, gy) = me.grid_pos;
        let neighbor_pos = match edge {
            Edge::Right => (gx.saturating_add(1), gy),
            Edge::Left => (gx.saturating_sub(1), gy),
            Edge::Bottom => (gx, gy.saturating_add(1)),
            Edge::Top => (gx, gy.saturating_sub(1)),
        };
        self.machines
            .iter()
            .find(|m| m.grid_pos == neighbor_pos)
    }

    pub fn opposite_edge(edge: Edge) -> Edge {
        match edge {
            Edge::Right => Edge::Left,
            Edge::Left => Edge::Right,
            Edge::Top => Edge::Bottom,
            Edge::Bottom => Edge::Top,
        }
    }

    pub fn next_free_grid_pos(&self) -> Option<(u8, u8)> {
        for y in 0..2u8 {
            for x in 0..2u8 {
                if !self.machines.iter().any(|m| m.grid_pos == (x, y)) {
                    return Some((x, y));
                }
            }
        }
        None
    }
}

fn config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "freemouse")
        .map(|d| d.config_dir().join("layout.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(w: f64, h: f64) -> ScreenInfo {
        ScreenInfo {
            x: 0.0,
            y: 0.0,
            width: w,
            height: h,
            primary: true,
        }
    }

    #[test]
    fn neighbor_adjacency() {
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();
        let layout = WorkspaceLayout {
            machines: vec![
                MachineLayout {
                    machine_id: id_a,
                    hostname: "A".into(),
                    ip: "10.0.0.1".into(),
                    grid_pos: (0, 0),
                    screens: vec![screen(1920.0, 1080.0)],
                },
                MachineLayout {
                    machine_id: id_b,
                    hostname: "B".into(),
                    ip: "10.0.0.2".into(),
                    grid_pos: (1, 0),
                    screens: vec![screen(1920.0, 1080.0)],
                },
            ],
        };
        let neighbor = layout.neighbor_at_edge(id_a, Edge::Right).unwrap();
        assert_eq!(neighbor.machine_id, id_b);
    }
}
